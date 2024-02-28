.PHONY: all
all: build

.PHONY: build
build:
	nix build --no-link ..#nginx

.PHONY: build
run:
	nix run ..#nginx -- -c `pwd`/nginx.conf -e /tmp/error.log
