#!/usr/bin/env python3
"""Jev screenshot judge: a first-pass visual gate for computer use.

Backed by Jev — TypeSafe AI's System One judgment model (NOT a
generative LLM). One call sends the screenshot as an image asset
plus two questions:
  present : noul — probability (0-1) that the target is visible
  locate  : box  — the target's region in the image, with confidence
and prints a normalized JSON verdict:

  {"model": "jev-latest",
   "present": 0.97,
   "box": {"top": ..., "bottom": ..., "left": ..., "right": ...,
           "confidence": ...},
   "center": {"x": <px>, "y": <px>},
   "usage": {...}}

center is in the image's own pixel space (the scaled screenshot
space, max width 1280), computed from the box. The box coordinate
scale (0-1 vs 0-1000) is detected from the returned values.

Jev is a gate, not the final word: in our measurements its box
overlapped the target well but its center could be off by ~10 px,
so the driving agent should verify with its own eyes before using
the coordinates to click.

Judging principle: when in doubt, pass — treat a middling `present`
probability as "look closer", never as a silent drop.

Usage:
  judge.py --image <png-or-jpg> --target "<what to look for>"
           [--model jev-latest] [--key-stdin]

API key, first match wins:
  --key-stdin            read one line from stdin (never stored)
  TYPESAFE_API_KEY env   read from the environment

Note: the image is sent to the TypeSafe API (api.typesafe.ai).
Do not judge screenshots that show passwords or private content
you have not agreed to send to a third party.
"""

import argparse
import base64
import json
import os
import sys
import urllib.request

HOST = "api.typesafe.ai"
BASE = f"https://{HOST}/v1"
DEFAULT_MODEL = "jev-latest"
ASSET_KEY = "screenshot_image"


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        raise RuntimeError("redirect refused, verify the endpoint")


def box_center(box, width, height):
    """Return the box center in pixels, detecting the value scale."""
    try:
        top = float(box["top"])
        bottom = float(box["bottom"])
        left = float(box["left"])
        right = float(box["right"])
    except (KeyError, TypeError, ValueError):
        return None
    peak = max(top, bottom, left, right)
    if peak <= 1.0:
        scale_x, scale_y = width, height
    elif peak <= 1000.0:
        scale_x, scale_y = width / 1000.0, height / 1000.0
    else:  # already pixels
        scale_x = scale_y = 1.0
    return {
        "x": int((left + right) / 2.0 * scale_x),
        "y": int((top + bottom) / 2.0 * scale_y),
    }


def main():
    parser = argparse.ArgumentParser(description="Jev screenshot judge (TypeSafe System One)")
    parser.add_argument("--image", required=True)
    parser.add_argument("--target", required=True, help="what to look for in the screenshot")
    parser.add_argument("--model", default=DEFAULT_MODEL)
    parser.add_argument(
        "--key-stdin",
        action="store_true",
        help="read the TypeSafe API key from stdin and use it transiently (no storage)",
    )
    args = parser.parse_args()

    if args.key_stdin:
        key = sys.stdin.readline().strip()
        if not key:
            print(json.dumps({"error": "no key on stdin"}))
            sys.exit(1)
    else:
        key = os.environ.get("TYPESAFE_API_KEY", "").strip()
        if not key:
            print(
                json.dumps(
                    {
                        "error": "no TypeSafe API key: set TYPESAFE_API_KEY or pass --key-stdin"
                    }
                )
            )
            sys.exit(1)

    fmt = "jpeg" if args.image.lower().endswith((".jpg", ".jpeg")) else "png"
    with open(args.image, "rb") as f:
        data_b64 = base64.b64encode(f.read()).decode("ascii")

    try:
        from PIL import Image
        import io

        with Image.open(io.BytesIO(base64.b64decode(data_b64))) as im:
            width, height = im.size
    except Exception:
        width = height = None

    body = {
        "model": args.model,
        "state": f"The screenshot to judge is {ASSET_KEY}.",
        "assets": {
            ASSET_KEY: {"type": "image", "format": fmt, "data_base64": data_b64}
        },
        "questions": {
            "present": {
                "type": "noul",
                "instructions": (
                    f"In the screenshot {ASSET_KEY}, is the following "
                    f"visible: {args.target}?"
                ),
            },
            "locate": {
                "type": "box",
                "instructions": (
                    f"In the screenshot {ASSET_KEY}, the region "
                    f"containing: {args.target}"
                ),
            },
        },
    }
    headers = {
        "Content-Type": "application/json",
        "TypeSafe-Danger-Zone-Images": "true",
        "Authorization": f"Bearer {key}",
    }
    request = urllib.request.Request(
        BASE + "/systemone",
        data=json.dumps(body).encode("utf-8"),
        headers=headers,
        method="POST",
    )
    with urllib.request.build_opener(NoRedirect()).open(request, timeout=120) as response:
        result = json.loads(response.read().decode("utf-8"))

    answers = result.get("answers", {})
    present = answers.get("present", {}).get("noul")
    box = answers.get("locate", {}).get("box") or answers.get("locate")
    center = None
    if isinstance(box, dict) and width and height:
        center = box_center(box, width, height)
    print(
        json.dumps(
            {
                "model": result.get("model", args.model),
                "present": present,
                "box": box,
                "center": center,
                "image_size": {"width": width, "height": height},
                "usage": result.get("usage", {}),
                "raw_answers": answers,
            },
            ensure_ascii=False,
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
