all: build

build:
	cargo build

run: build
	sudo ./target/debug/iou

test:
	sudo -E $(which cargo) test
	
clean:
	cargo clean
