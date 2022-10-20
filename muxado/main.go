package main

import (
	"bufio"
	"io"
	"log"
	"net"
	"os"
	"strings"
	"sync"
	"time"

	"github.com/inconshreveable/muxado"
	"github.com/inconshreveable/muxado/frame"
)

func main() {
	log.Println("reqeusted ", os.Args[1], " mode")
	if os.Args[1] == "client" {
		runClient(os.Args[2:])
	} else if os.Args[1] == "heartbeat" {
		runHeartbeat(os.Args[2:])
	} else {
		runServer(os.Args[2:])
	}
}

func runClient(args []string) {
	conn, err := net.Dial("tcp", "localhost:1234")
	if err != nil {
		log.Fatal("Failed to establish tcp connection:", err)
	}

	log.Println("TCP Connected")

	sess := muxado.Client(conn, &muxado.Config{
		NewFramer: func(r io.Reader, w io.Writer) frame.Framer {
			return frame.NewDebugFramer(os.Stdout, frame.NewFramer(r, w))
		},
	})

	g := sync.WaitGroup{}

	for i := 0; i < 256; i++ {
		for _, s := range [][]string{
			{"foo", "bar", "baz"},
			{"qas", "wex", "exhort"},
		} {
			g.Add(1)
			go func(s []string) {
				stream, err := sess.OpenStream()
				if err != nil {
					log.Fatal("Failed to open stream:", err)
				}

				log.Println("Opened stream")

				g2 := sync.WaitGroup{}

				g2.Add(1)
				go func() {
					buf := bufio.NewReader(stream)
					for {
						line, err := buf.ReadString('\n')
						if err != nil {
							// log.Print("Failed to read line:", err)
							g2.Done()
							return
						}
						line = strings.TrimRight(line, "\n")
						// log.Println("Read line:", line)
					}

				}()

			outer:
				for _, s := range s {
					bs := []byte(s + "\n")
					l := len(bs)
					n := 0
					for n < l {
						n, err = stream.Write(bs[n:])
						if err != nil {
							log.Println("Write error:", err)
							break outer
						}
					}
				}
				stream.CloseWrite()
				g2.Wait()
				stream.Close()
				g.Done()
			}(s)
		}
	}

	g.Wait()
	sess.Close()
	conn.Close()
}

func runServer(args []string) {
	l, err := net.Listen("tcp", "localhost:1234")
	if err != nil {
		log.Fatal(err)
	}

	g := sync.WaitGroup{}

	for {
		c, err := l.Accept()
		if err != nil {
			log.Println("Error accepting connection:", err)
			break
		}

		g.Add(1)
		go func() {
			handleConn(c)
			g.Done()
		}()
	}

	g.Wait()
}

func handleConn(c net.Conn) error {
	srv := muxado.Server(c, &muxado.Config{
		NewFramer: func(r io.Reader, w io.Writer) frame.Framer {
			return frame.NewDebugFramer(os.Stdout, frame.NewFramer(r, w))
		},
	})

	g := sync.WaitGroup{}

	for {
		stream, err := srv.AcceptStream()
		if err != nil {
			log.Println("Error accepting stream:", err)
			break
		}

		g.Add(1)
		go func() {
			err := handleStream(stream)
			log.Println("Stream closed:", err)
			g.Done()
		}()
	}

	g.Wait()
	return nil
}

func handleStream(stream muxado.Stream) error {
	defer stream.CloseWrite()
	buf := make([]byte, 128)
	for {
		r, err := stream.Read(buf)
		if err != nil {
			return err
		}

		n := 0
		for n < r {
			n, err = stream.Write(buf[n:r])
			if err != nil {
				return err
			}
		}
	}
}

func runHeartbeat(args []string) {
	log.Println("starting heartbeat listener")
	l, err := net.Listen("tcp", "localhost:1234")
	if err != nil {
		log.Fatal(err)
	}

	for {
		log.Println("waiting for heartbeat client")
		c, err := l.Accept()
		if err != nil {
			log.Println("Error accepting connection:", err)
			break
		}

		srv := muxado.Server(c, &muxado.Config{
			NewFramer: func(r io.Reader, w io.Writer) frame.Framer {
				return frame.NewDebugFramer(os.Stdout, frame.NewFramer(r, w))
			},
		})

		typed := muxado.NewTypedStreamSession(srv)

		heartbeat := muxado.NewHeartbeat(typed, func(d time.Duration) {
			println("got heartbeat: ", d)
		}, muxado.NewHeartbeatConfig())

		heartbeat.AcceptTypedStream()
	}
}
