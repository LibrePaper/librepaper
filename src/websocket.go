package main

import (
	"bufio"
	"crypto/sha1"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"strings"
	"sync"
)

// A minimal RFC 6455 server: text frames, ping/pong, and close. That is the
// whole protocol the reader uses, and writing it here keeps the binary free of
// dependencies.

const wsGUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

// wsMaxPayload bounds one message. Comments are capped far below this; the
// limit exists so a hostile client cannot ask us to buffer a gigabyte.
const wsMaxPayload = 1 << 20

const (
	opContinuation = 0x0
	opText         = 0x1
	opClose        = 0x8
	opPing         = 0x9
	opPong         = 0xA
)

type wsConn struct {
	conn   net.Conn
	reader *bufio.Reader

	write  sync.Mutex
	closed bool
}

// wsUpgrade completes the handshake and hands back the raw connection. The
// http.ResponseWriter must not be used afterwards.
func wsUpgrade(w http.ResponseWriter, r *http.Request) (*wsConn, error) {
	if !strings.EqualFold(r.Header.Get("Upgrade"), "websocket") {
		return nil, errors.New("not a websocket upgrade")
	}
	key := r.Header.Get("Sec-WebSocket-Key")
	if key == "" {
		return nil, errors.New("missing Sec-WebSocket-Key")
	}
	hijacker, ok := w.(http.Hijacker)
	if !ok {
		return nil, errors.New("connection cannot be hijacked")
	}
	conn, buffered, err := hijacker.Hijack()
	if err != nil {
		return nil, err
	}

	sum := sha1.Sum([]byte(key + wsGUID))
	accept := base64.StdEncoding.EncodeToString(sum[:])
	response := "HTTP/1.1 101 Switching Protocols\r\n" +
		"Upgrade: websocket\r\n" +
		"Connection: Upgrade\r\n" +
		"Sec-WebSocket-Accept: " + accept + "\r\n\r\n"
	if _, err := io.WriteString(conn, response); err != nil {
		conn.Close()
		return nil, err
	}
	return &wsConn{conn: conn, reader: buffered.Reader}, nil
}

// readMessage returns the next text payload, answering pings and honouring
// close frames along the way. Binary frames are ignored, as the reader never
// sends them.
func (c *wsConn) readMessage() ([]byte, error) {
	var message []byte
	var messageOp byte

	for {
		final, opcode, payload, err := c.readFrame()
		if err != nil {
			return nil, err
		}

		switch opcode {
		case opClose:
			return nil, io.EOF
		case opPing:
			if err := c.writeFrame(opPong, payload); err != nil {
				return nil, err
			}
			continue
		case opPong:
			continue
		case opContinuation:
			if messageOp == 0 {
				return nil, errors.New("continuation without a start frame")
			}
		default:
			messageOp = opcode
			message = nil
		}

		message = append(message, payload...)
		if len(message) > wsMaxPayload {
			return nil, errors.New("message too large")
		}
		if !final {
			continue
		}
		if messageOp != opText {
			message, messageOp = nil, 0
			continue
		}
		return message, nil
	}
}

func (c *wsConn) readFrame() (bool, byte, []byte, error) {
	var header [2]byte
	if _, err := io.ReadFull(c.reader, header[:]); err != nil {
		return false, 0, nil, err
	}
	final := header[0]&0x80 != 0
	opcode := header[0] & 0x0F
	masked := header[1]&0x80 != 0
	length := uint64(header[1] & 0x7F)

	switch length {
	case 126:
		var extended [2]byte
		if _, err := io.ReadFull(c.reader, extended[:]); err != nil {
			return false, 0, nil, err
		}
		length = uint64(binary.BigEndian.Uint16(extended[:]))
	case 127:
		var extended [8]byte
		if _, err := io.ReadFull(c.reader, extended[:]); err != nil {
			return false, 0, nil, err
		}
		length = binary.BigEndian.Uint64(extended[:])
	}
	if length > wsMaxPayload {
		return false, 0, nil, errors.New("frame too large")
	}

	// Every frame from a client must be masked.
	var mask [4]byte
	if masked {
		if _, err := io.ReadFull(c.reader, mask[:]); err != nil {
			return false, 0, nil, err
		}
	}
	payload := make([]byte, length)
	if _, err := io.ReadFull(c.reader, payload); err != nil {
		return false, 0, nil, err
	}
	if masked {
		for i := range payload {
			payload[i] ^= mask[i%4]
		}
	}
	return final, opcode, payload, nil
}

func (c *wsConn) writeText(payload []byte) error {
	return c.writeFrame(opText, payload)
}

// writeFrame sends one unfragmented frame. A server never masks.
func (c *wsConn) writeFrame(opcode byte, payload []byte) error {
	c.write.Lock()
	defer c.write.Unlock()
	if c.closed {
		return errors.New("connection closed")
	}

	header := []byte{0x80 | opcode}
	size := len(payload)
	switch {
	case size < 126:
		header = append(header, byte(size))
	case size <= 0xFFFF:
		header = append(header, 126, byte(size>>8), byte(size))
	default:
		header = append(header, 127)
		var extended [8]byte
		binary.BigEndian.PutUint64(extended[:], uint64(size))
		header = append(header, extended[:]...)
	}
	if _, err := c.conn.Write(append(header, payload...)); err != nil {
		return err
	}
	return nil
}

// close sends a close frame and drops the connection. Repeated calls are safe.
func (c *wsConn) close(code uint16, reason string) {
	c.write.Lock()
	if c.closed {
		c.write.Unlock()
		return
	}
	c.write.Unlock()

	payload := make([]byte, 2, 2+len(reason))
	binary.BigEndian.PutUint16(payload, code)
	payload = append(payload, reason...)
	_ = c.writeFrame(opClose, payload)

	c.write.Lock()
	c.closed = true
	c.write.Unlock()
	_ = c.conn.Close()
}

func (c *wsConn) String() string {
	return fmt.Sprintf("ws:%s", c.conn.RemoteAddr())
}
