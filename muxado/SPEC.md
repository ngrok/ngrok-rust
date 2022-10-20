# Frame Layout

Unless otherwise specified, integers are stored big-endian.

Length: 24-bit unsigned integer
Type: 4-bit unsigned integer
Flags: 4-bit bitfield. Only used in DATA
StreamID: 32-bit unsigned integer
Body: Byte array of size Length

    +-----------------------+
0-3 |      Length     |T |F |
    +-----------------+--+--+
4-7 |        StreamID       |
    +-----------------------+
8-L |         Body          |
    |                       |
    |                       |
    |                       |
    +-----------------------+

## Frame Types

* RST
  Type: 0x0
  StreamID: non-zero
  Length: 4
  Body: 32-bit unsigned error code

* DATA
  Type: 0x1
  StreamID: non-zero
  Length: Variable
  Body: Byte array with given Length

* WNDINC
  Type: 0x2
  StreamID: non-zero
  Length: 4
  Body: 32-bit unsigned increment

* GOAWAY
  Type: 0x3
  StreamID: 0
  Length: 8
  Body: 32-bit stream ID, 32-bit error code
