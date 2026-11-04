# Python Bindings

The separately versioned [Python bindings](https://github.com/earldennison/pactree-py)
provide the `pactree` Python package. They marshal calls to this Rust engine;
they do not implement a second storage or execution model. Source builds use
matching Rust and Python checkouts in sibling directories.

Python integration boundaries:

- use the Python package for native in-process calls, or `pactree` CLI commands
  for process-boundary workflows;
- pass PACT JSON, root-store JSON, trace JSON, and manifest JSON over stdin or
  files;
- keep Pactree terms intact at the boundary;
- translate host-specific names only inside the adapter layer.
