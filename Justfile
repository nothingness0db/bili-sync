clean:
    rm -rf ./bili-eye-Linux*.tar.gz

build-frontend:
    cd ./web && bun run build && cd ..

build: build-frontend
    cargo build --target x86_64-unknown-linux-musl --release

build-debug: build-frontend
    cargo build --target x86_64-unknown-linux-musl

build-docker: build
    tar czvf ./bili-eye-Linux-x86_64-musl.tar.gz -C ./target/x86_64-unknown-linux-musl/release/ ./bili-eye
    docker build . -t bili-eye-local --build-arg="TARGETPLATFORM=linux/amd64"
    just clean

build-docker-debug: build-debug
    tar czvf ./bili-eye-Linux-x86_64-musl.tar.gz -C ./target/x86_64-unknown-linux-musl/debug/ ./bili-eye
    docker build . -t bili-eye-local --build-arg="TARGETPLATFORM=linux/amd64"
    just clean

debug: build-frontend
    cargo run
