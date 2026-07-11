FROM rust:1-slim
RUN apt-get update -qq && apt-get install -y -qq libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*
WORKDIR /workspace/vialo
CMD ["sleep", "infinity"]
