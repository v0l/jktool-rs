ARG TARGETARCH
FROM --platform=linux/${TARGETARCH} rust:trixie as builder

# Install system dependencies including dbus
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
    pkg-config \
    libdbus-1-dev \
    clang \
    llvm \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy manifest files first for better caching
COPY Cargo.toml Cargo.lock ./
COPY src/ ./src/

# Build the binary
RUN cargo build --release --features bluetooth

# Extract the binary
RUN mkdir -p /output && \
    cp /build/target/release/jktool /output/
