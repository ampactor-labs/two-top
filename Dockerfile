# The ice_vendor credential service, and nothing else — Railway builds
# this on `railway up` / repo deploys. Multi-stage: the workspace compiles
# in the rust image, the runtime ships one binary. ca-certificates covers
# the vendor's outbound HTTPS call to Cloudflare's credential generator.
# The game binaries never touch this file.
FROM rust:1.95-slim AS build
WORKDIR /src
COPY . .
RUN cargo build -p ice_vendor --release --locked

FROM debian:stable-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/ice_vendor /usr/local/bin/ice_vendor
ENV PORT=8080
EXPOSE 8080
CMD ["ice_vendor"]
