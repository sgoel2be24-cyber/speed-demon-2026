# Profile-guided build. /data is not available here, so the training run uses a dataset from
# the organisers' generator, on a seed that is neither the public one nor tuned for.
FROM python:3.13-slim@sha256:d98cc29d3266e535dd9da1099688d9cb3ca4753813571e071763d17725460962 AS training
COPY generator/b_index.py /b_index.py
RUN python3 /b_index.py --seed 7 --scale sample --out /training

FROM halloumee/speed-demon-rust:1.99.1-trixie-llvm-tools@sha256:13e4ee7135d8eac6c56476c7d1aee201f36479f4745fd5d0e6b4529f4e13251a AS build
WORKDIR /src
COPY . .
COPY --from=training /training /training
RUN cargo rustc --release --offline -- -C profile-generate=/pgo \
 && ./target/release/solution /training /tmp 12 \
 && llvm-profdata merge -o /pgo.profdata /pgo \
 && touch src/main.rs \
 && cargo rustc --release --offline -- -C profile-use=/pgo.profdata

FROM debian:13-slim@sha256:62bd423aeee912639ca8ee681c2b762cb3e25137e0a92b20be455898a9f034ad
COPY --from=build /src/target/release/solution /app/solution
ENTRYPOINT ["/app/solution", "/data", "/out", "6"]
