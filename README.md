# Env variables:
- BIND_ADDR(default: 0.0.0.0:25565): the address the server should bind to
- FILTER_CONN(default: '(addr == "10.100.0.1")'): the filter appplied with [evalexpr](https://docs.rs/evalexpr/latest/evalexpr/)
  The "context" has the addr variable populated with the address of part of the
  handshake packet. Custom filter can be specified, and drop any
  connections which match the filter.
