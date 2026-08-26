# Vendored netwatch 0.19.1

This directory is the `netwatch` 0.19.1 crate from
<https://github.com/n0-computer/net-tools>, distributed under its declared
`MIT OR Apache-2.0` license. PEAT elects the Apache-2.0 option; the license text
is in `LICENSE-APACHE`.

The local delta adds an exclusive, fail-closed UDP pre-bind hook. Android uses
that generic seam to apply an OS network handle to PEAT/Iroh-owned sockets
without process-wide routing. The hook runs on initial bind and rebind.

No Meshrabiya source or dependency is included.
