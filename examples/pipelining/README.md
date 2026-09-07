# RTC pipelining

This example compares ordinary sequential calls with pipelined calls over an
in-memory connection with 150 ms of simulated latency in each direction.

Both runs open a remote counter, increase it and read its value. The sequential
run waits for every result and therefore takes three round trips. The pipelined
run creates the counter client locally and sends all three requests before
waiting for any result, so it takes one round trip.

Run it from the top-level repository directory:

    cargo run --manifest-path examples/pipelining/Cargo.toml

The exact measured times vary slightly, but should be close to 900 ms without
pipelining and 300 ms with pipelining.
