# The tape drop's deploy image (NORTH N3). Railway builds this because
# .railway/railway.ts names it as tape-drop's Dockerfile. Left to guess,
# Railway builds the root Dockerfile instead, which is ice_vendor's: the
# first deploy shipped exactly that.
FROM rust:1.95 AS build
WORKDIR /src
COPY . .
RUN cargo build -p tape_drop --release --locked

FROM debian:bookworm-slim
COPY --from=build /src/target/release/tape_drop /usr/local/bin/tape_drop
ENV PORT=8080
CMD ["tape_drop"]
