# ngrok-nginx

This is a very-wip nginx module that adds ngrok support.

`libngrok-nginx` contains the Rust code that imports the core ngrok library and
exposes a C-compatible interface fit for consumption by an nginx module.

`nginx-module` contains the actual module in question. See
`nginx-module/config` for the defintions consumed by the nginx build system to
compile the module.

`nginx-headers` contains the headers from nginx-1.24.0 using their original file
structure. These are provided primarily to assist in development. Configure your
IDE to include them in its set of include paths to get intellisense working.
Suggested settings for VSCode can be found in `.vscode/settings.json.example`.

## Building

`libngrok-nginx` can be built with `cargo build` as one might expect. This is
only really needed for development though.

The module needs to be compiled by nginx itself. Rather than vendor the full
nginx source here, nix definitions to build it are provided instead. From the
root of the repository:

```bash
nix build .#nginx
```

And that's it! It will build the Rust library, and then nginx and the module,
which will be statically linked.

## Configuring

See `ngrok-nginx/nginx.conf` for an example configuration.
