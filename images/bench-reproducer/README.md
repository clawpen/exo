# Exo benchmark reproducer

A one-shot container that runs the Exo-vs-Docker benchmark suite and writes the
results to a mounted directory. Use it to verify the Exo Enterprise efficiency
claims without installing Rust or building from source.

## Build

Run from the repository root:

```bash
docker build -t exo-bench-reproducer:latest -f images/bench-reproducer/Containerfile .
```

The build compiles the `exo` release binary inside the image.

## Run

Mount the host Docker socket (so the container can measure Docker) and an output
folder for results:

```bash
mkdir -p results
docker run --rm \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v $(pwd)/results:/results \
  exo-bench-reproducer:latest
```

## Output

The container writes:

- `results/bench-*.json` — head-to-head and density raw results
- `results/summary.md` — combined Markdown report (when PowerShell is available)

## Notes

- The container uses `docker:27-dind-rootless` as its base. It does **not** need
to run as privileged if you mount the host Docker socket.
- If the Docker socket is not available, Docker-side numbers are skipped and only
Exo numbers are produced.
