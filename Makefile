.PHONY: build release linux upload clean

LINUXDIR := target/x86_64-unknown-linux-musl/release
TARGET := iperf

build:
	cargo build

release:
	cargo build --release

linux:
	cargo zigbuild --release --target x86_64-unknown-linux-musl

upload: linux
	curl -X POST "http://update.scythefly.top:61910/upload?dst=binary/iperf" \
		-F "file=@${LINUXDIR}/${TARGET}" \
		-H "Content-Type: multipart/form-data"

clean:
	cargo clean
