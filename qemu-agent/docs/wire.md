# Wire protocol

The guest agent and the host talk over a single byte stream (a virtserialport
channel). That one stream carries multiple logical channels — shell stdin,
stdout, stderr, and control events — multiplexed as simple length-prefixed
frames.

## Frame layout

All integers are little-endian.

```text
+------------+----------------+----------+
| type:1     | payload_len:2  | payload  |
+------------+----------------+----------+
```

- `type` — one byte identifying the frame type (see below).
- `payload_len` — `u16` length of the payload only; it does not include the
  3-byte header.
- Maximum frame size is 16384 bytes (`MAX_FRAME_SIZE`), so the payload is
  capped at 16381 bytes (`MAX_PAYLOAD`). Oversized payloads are rejected on
  both write and read with `InvalidInput`.

The underlying channel is assumed reliable and ordered: there is no CRC, no
magic byte, and no resynchronization mechanism.

## Frame types

| Value | Type     | Payload                           | Handled by |
| ----- | -------- | --------------------------------- | ---------- |
| 1     | `Start`  | —                                 | —          |
| 2     | `Stdin`  | raw bytes for the shell's stdin   | server     |
| 3     | `Stdout` | raw bytes from the shell's stdout | client     |
| 4     | `Stderr` | raw bytes from the shell's stderr | client     |
| 5     | `Resize` | `cols:u16, rows:u16` (4 bytes)    | server     |

Any other type value decodes as `Unknown`. The frame is still read in full so
the stream stays in sync; the receiver decides whether to drop it.

## Stream semantics

- Frames are read sequentially; a clean EOF at a frame boundary ends the
  stream normally.
- EOF in the middle of a frame (header or payload) is an error
  (`UnexpectedEof`).
- Empty payloads are valid.
