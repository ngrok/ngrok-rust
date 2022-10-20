module github.com/jrobsonchase/muxado

go 1.16

require (
	github.com/hashicorp/yamux v0.0.0-20211028200310-0bc27b27de87 // indirect
	github.com/inconshreveable/muxado v0.0.0-20160802230925-fc182d90f26e
	golang.org/x/crypto v0.0.0-20211117183948-ae814b36b871 // indirect
)

replace github.com/inconshreveable/muxado => ./replace/muxado
