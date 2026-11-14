# Revision A serial protocol

This specification describes the exchanges used by turing-screen-monitor for a
revision A 3.5-inch display in 480 × 320 landscape. Other device revisions are
outside its scope. All numbers below are decimal unless prefixed with `0x`.

## Transport and discovery

Use 115200 baud, eight data bits, no parity, one stop bit, hardware RTS/CTS flow
control and a two-second read/write timeout. Automatic discovery selects an entry
in `/dev/serial/by-id` whose name contains `USB35INCHIPSV2`. No match is an error;
multiple matches require an explicit device path. Discovery is repeated when
reconnecting. The supported USB ID is `1a86:5722`.

## Connection sequence

Clear pending input, then transmit six bytes of `0x45`. Read up to six response
bytes, accumulating fragmented reads until six bytes, EOF or a timeout. Exactly
six `0x01` bytes identify a supported model. Receiving no bytes is also accepted
for models that remain silent. Any other nonempty response, including a truncated
response, is rejected before sending configuration or pixels. Other I/O errors
abort the connection. There is no reset or USB re-enumeration command.

After acceptance, send orientation, brightness and screen-on packets, in that
order. The next image is a full frame. Configuration is sent again on reconnect.

## Six-byte command packet

Four unsigned ten-bit fields describe inclusive rectangle coordinates: left X,
top Y, right X and bottom Y. Pack them consecutively into a 40-bit integer, most
significant field first, followed by the eight-bit command code:

```text
byte       7                    0
  0        X[9:2]
  1        X[1:0]   Y[9:4]
  2        Y[3:0]   right[9:6]
  3        right[5:0]   bottom[9:8]
  4        bottom[7:0]
  5        command[7:0]

bits 39..30 = X; 29..20 = Y; 19..10 = right; 9..0 = bottom
```

Screen-on uses command 109 (`0x6d`) and four zero fields.
Brightness uses command 110 (`0x6e`), with only X populated. For a percentage
`p` in 0..100, X is `truncate(255.0 - p / 100.0 * 255.0)`. Thus 0% gives 255,
20% gives 204, 25% gives 191 and 100% gives zero: the device scale is inverted.
Out-of-range brightness is rejected before opening the transport.

A bitmap rectangle uses command 197 (`0xc5`) followed immediately by its pixels.
Coordinates must lie within X=0..479 and Y=0..319. Width and height include both
endpoints. A full frame starts with `00 00 07 7d 3f c5` (hexadecimal).
The rectangle X=10, Y=20, width=100, height=200 starts with
`02 81 41 b4 db c5`.

## Orientation packet

Orientation is a separate 16-byte packet. Bytes 0..4 are zero, byte 5 is 121
(`0x79`), and byte 6 is 102 for landscape or 103 for reversed landscape.
Bytes 7..8 contain width 480 in big-endian order (`01 e0`); bytes 9..10
contain height 320 (`01 40`). Bytes 11..15 are zero. Reversing orientation
keeps these dimensions and all bitmap coordinate rules.

## Pixel stream and incremental updates

Pixels are row-major, left to right then top to bottom, with no padding.
Convert RGB888 to RGB565 by discarding the low three red bits, low two green
bits and low three blue bits. Bits 15..11 are red, 10..5 green, 4..0 blue.
Transmit each 16-bit pixel least significant byte first. Full-intensity red,
green and blue therefore encode as `00 f8`, `e0 07` and `1f 00`.

Each rectangle contains exactly width × height × 2 pixel bytes. Write pixel
payloads in chunks of at most 3840 bytes (480 × 8), without intervening headers,
checksums, padding or acknowledgements. Flush after all rectangles in an update.
A full frame is 307206 bytes including its six-byte header.

The first frame covers the entire display. Later frames group consecutive
changed rows into rectangles bounded by their changed columns. Unchanged frames
send no rectangles. An optional full-width mode expands each rectangle to X=0..479.
Cache the image only after a successful flush; an I/O failure requires a fresh
connection and full frame so incomplete writes cannot leave a stale baseline.
