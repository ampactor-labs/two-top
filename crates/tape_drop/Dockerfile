# The tape drop's deploy image (NORTH N3). Railway builds this when the
# service's config points build.dockerfilePath here — the workspace's
# multi-bin layout confuses buildpack heuristics (the first attempt
# auto-built ice_vendor instead), and a Dockerfile has no heuristics.
FROM rust:1.95 AS build
WORKDIR /src
COPY . .
RUN cargo build -p tape_drop --release --locked

FROM debian:bookworm-slim
COPY --from=build /src/target/release/tape_drop /usr/local/bin/tape_drop
ENV PORT=8080
CMD ["tape_drop"]
