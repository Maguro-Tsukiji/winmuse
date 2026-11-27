# server/ — legacy Python implementation (reference only)

This is the original Python Computer Use bridge, kept for
reference. It is **not** the product and is no longer maintained.

It has **no permission profiles, no bearer-token authentication,
and no audit log**: anything that can reach its endpoint can
operate the PC without restriction. Do not run it on a network
you do not fully control, and do not mistake it for the protected
server.

The supported server is the Rust implementation in
[`../winmuse/`](../winmuse/) — tray-resident, with permission
profiles, an optional bearer token, an audit log, and a settings
GUI. Its configuration example lives at
[`../winmuse/config.example.toml`](../winmuse/config.example.toml).
