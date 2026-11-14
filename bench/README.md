# Benchmarks

## Agent

`agent_load.py` starts the agent binary on a spare port (27015 by default), attaches fake viewers that speak the wire protocol, and reports for each scenario:
- the agent's CPU, measured from its exact CPU time;
- its resident memory;
- the frame size;
- how many frames each viewer received;
- the gaps between frames (p50, p99, max).

```bash
python3 -I bench/agent_load.py target/release/magi
```

`--stress` repeats the viewer scenarios while every core runs a busy loop (`yes`) and up to `--sleepers` extra processes sleep. The number of sleepers is capped at 60% of your process limit. Every process it starts is killed afterwards.

```bash
python3 -I bench/agent_load.py target/release/magi --stress --seconds 15
```

## Viewer

Render time per frame (fleets of 2 and 60 machines, desktop and phone sizes), model update cost, and the bytes ratatui writes to the terminal per update:

```bash
cargo test --release -- --ignored --nocapture bench
```
