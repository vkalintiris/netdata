Netdata's protocol for external plugins:
  - is difficult to use,
  - is error prone,
  - is antiquated,
  - is non-transferable to other domains,
  - has been developed over time
    - started as text/line oriented protocol
    - evolved to support multiple-line messages and binary data
  - under-documented and wrongly-documented.

The plan is to drop the custom protocol, at some point, and replace it with
regular gRPC services.

While the protocol is low-level and primitive, it does support some high-level
operations. For example:
  - Function calls with handles that support timeouts and cancellation,
  - Config management commands, etc.

I have to develop an external plugin in Rust, that will use most of the protocol's
messages. The plugin needs to provide a big amount of features and functionality.

I wrote a tokio codec that can be used to abstract the intricacies of Netdata's
protocol and creates Framed messages.

I want to create a netdata-plugin-runtime crate, that will allow me to focus
on the bussiness logic while developing my plugin.

I think that I can do that if I model the plugin as a grpc service and provide
a plugin runtime that will offer a nice API to the authors of the plugins.

I was thinking that the runtime should contain the plugin gRPC client and the gRPC
server. The gRPC connection should use an in-memory stream/channel.

The plugin runtime:
    - Should use the transport to parse messages from stdin and use the client
      to perform the corresponding request.
    - The server should perform any logic required and the runtime should emit
      to stdout the responses for the agent to read.
    - Should provide a function registry so that the plugin can register the
      function calls it supports prior to running the runtime. IIUC, this should
      be a hashmap from function names to async function handlers through
      type-erasure.
    - Should provide a transaction registry to keep around function calls that
      are executed asynchronously.

For the time being, we are only interested in supporting:
    - Function declaration and calls with timeouts and cancellation.
    - A plugin and function context that function handlers will receive in
      their arguments so that they are able to modify/mutate the plugin's state.

I'm not interested in tests. However, I want you to provide, for future reference,
an example that will spawn the runtime and work as intended.
