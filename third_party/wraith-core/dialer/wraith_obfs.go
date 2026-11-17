package dialer

// Wraith: Xray-style fragmentation and UDP noise for the connection to the proxy server.
//
// sing-box only fragments the TLS ClientHello at SNI boundaries (tls.fragment). Happ, which runs
// Xray, exposes range-based fragmentation (length / interval / max split, "tlshello" or the first
// N writes) and noise packets sent before the first UDP datagram. Both are implemented here as a
// wrapper around the outbound's dialer, enabled per outbound by the wraith_fragment and
// wraith_noise dialer options. Without those options the dialer is returned untouched.

import (
	"context"
	"crypto/rand"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"math/big"
	"net"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/sagernet/sing-box/option"
	E "github.com/sagernet/sing/common/exceptions"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
)

type wraithRange struct{ from, to int64 }

func parseWraithRange(value string, fallback wraithRange) (wraithRange, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return fallback, nil
	}
	parts := strings.SplitN(value, "-", 2)
	from, err := strconv.ParseInt(strings.TrimSpace(parts[0]), 10, 64)
	if err != nil {
		return wraithRange{}, E.New("bad range: ", value)
	}
	to := from
	if len(parts) == 2 {
		to, err = strconv.ParseInt(strings.TrimSpace(parts[1]), 10, 64)
		if err != nil {
			return wraithRange{}, E.New("bad range: ", value)
		}
	}
	if from < 0 || to < from {
		return wraithRange{}, E.New("bad range: ", value)
	}
	return wraithRange{from, to}, nil
}

func (r wraithRange) pick() int64 {
	if r.to <= r.from {
		return r.from
	}
	n, err := rand.Int(rand.Reader, big.NewInt(r.to-r.from+1))
	if err != nil {
		return r.from
	}
	return r.from + n.Int64()
}

type wraithFragment struct {
	tlsHello    bool
	first, last int64
	length      wraithRange
	interval    wraithRange
	maxSplit    wraithRange
}

type wraithNoise struct {
	kind      string
	packet    []byte
	delay     wraithRange
	randLen   wraithRange
	randValue wraithRange
}

func buildWraithFragment(options *option.WraithFragmentOptions) (*wraithFragment, error) {
	if options == nil {
		return nil, nil
	}
	f := &wraithFragment{}
	packets := strings.ToLower(strings.TrimSpace(options.Packets))
	if packets == "" || packets == "tlshello" {
		f.tlsHello = true
	} else {
		r, err := parseWraithRange(packets, wraithRange{})
		if err != nil || r.from < 1 {
			return nil, E.New("wraith_fragment.packets: tlshello or a write range like 1-3")
		}
		f.first, f.last = r.from, r.to
	}
	var err error
	if f.length, err = parseWraithRange(options.Length, wraithRange{50, 100}); err != nil {
		return nil, E.Cause(err, "wraith_fragment.length")
	}
	if f.length.from < 1 {
		return nil, E.New("wraith_fragment.length must be positive")
	}
	if f.interval, err = parseWraithRange(options.Interval, wraithRange{10, 20}); err != nil {
		return nil, E.Cause(err, "wraith_fragment.interval")
	}
	if f.maxSplit, err = parseWraithRange(options.MaxSplit, wraithRange{0, 0}); err != nil {
		return nil, E.Cause(err, "wraith_fragment.max_split")
	}
	return f, nil
}

func buildWraithNoises(options []option.WraithNoiseOptions) ([]wraithNoise, error) {
	var noises []wraithNoise
	for i, o := range options {
		n := wraithNoise{kind: strings.ToLower(strings.TrimSpace(o.Type))}
		var err error
		if n.delay, err = parseWraithRange(o.Delay, wraithRange{10, 16}); err != nil {
			return nil, E.Cause(err, "wraith_noise[", i, "].delay")
		}
		switch n.kind {
		case "rand":
			if n.randLen, err = parseWraithRange(o.Rand, wraithRange{1, 8192}); err != nil || n.randLen.from < 1 {
				return nil, E.New("wraith_noise[", i, "].rand: length range like 1-8192")
			}
			if n.randValue, err = parseWraithRange(o.RandRange, wraithRange{0, 255}); err != nil || n.randValue.to > 255 {
				return nil, E.New("wraith_noise[", i, "].rand_range: byte range within 0-255")
			}
		case "str":
			n.packet = []byte(o.Packet)
		case "base64":
			n.packet, err = base64.StdEncoding.DecodeString(strings.TrimSpace(o.Packet))
			if err != nil {
				return nil, E.Cause(err, "wraith_noise[", i, "].packet")
			}
		case "hex":
			n.packet, err = hex.DecodeString(strings.TrimSpace(o.Packet))
			if err != nil {
				return nil, E.Cause(err, "wraith_noise[", i, "].packet")
			}
		case "array", "":
			// Happ's "array" form: comma-separated hex bytes, e.g. 1A,2B,3C.
			n.kind = "array"
			for _, item := range strings.Split(o.Packet, ",") {
				item = strings.TrimSpace(item)
				if item == "" {
					continue
				}
				b, perr := strconv.ParseUint(strings.TrimPrefix(strings.ToLower(item), "0x"), 16, 8)
				if perr != nil {
					return nil, E.New("wraith_noise[", i, "].packet: comma-separated hex bytes like 1A,2B,3C")
				}
				n.packet = append(n.packet, byte(b))
			}
		default:
			return nil, E.New("wraith_noise[", i, "].type: array, rand, str, base64 or hex")
		}
		if n.kind != "rand" && len(n.packet) == 0 {
			return nil, E.New("wraith_noise[", i, "].packet is empty")
		}
		noises = append(noises, n)
	}
	return noises, nil
}

func (n wraithNoise) generate() []byte {
	if n.kind != "rand" {
		return n.packet
	}
	out := make([]byte, n.randLen.pick())
	for i := range out {
		out[i] = byte(n.randValue.pick())
	}
	return out
}

// wrapWraithObfs returns the dialer unchanged unless the outbound asked for fragmentation or noise.
func wrapWraithObfs(dialer N.Dialer, options option.DialerOptions) (N.Dialer, error) {
	fragment, err := buildWraithFragment(options.WraithFragment)
	if err != nil {
		return nil, err
	}
	noises, err := buildWraithNoises(options.WraithNoise)
	if err != nil {
		return nil, err
	}
	if fragment == nil && len(noises) == 0 {
		return dialer, nil
	}
	return &wraithDialer{Dialer: dialer, fragment: fragment, noises: noises}, nil
}

type wraithDialer struct {
	N.Dialer
	fragment *wraithFragment
	noises   []wraithNoise
}

func (d *wraithDialer) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	conn, err := d.Dialer.DialContext(ctx, network, destination)
	if err != nil {
		return nil, err
	}
	switch N.NetworkName(network) {
	case N.NetworkTCP:
		if d.fragment != nil {
			return &wraithFragmentConn{Conn: conn, fragment: d.fragment}, nil
		}
	case N.NetworkUDP:
		if len(d.noises) > 0 {
			return &wraithNoiseConn{Conn: conn, noises: d.noises}, nil
		}
	}
	return conn, nil
}

func (d *wraithDialer) ListenPacket(ctx context.Context, destination M.Socksaddr) (net.PacketConn, error) {
	conn, err := d.Dialer.ListenPacket(ctx, destination)
	if err != nil || len(d.noises) == 0 {
		return conn, err
	}
	return &wraithNoisePacketConn{PacketConn: conn, noises: d.noises, sent: make(map[string]bool)}, nil
}

func (d *wraithDialer) Upstream() any { return d.Dialer }

// wraithFragmentConn splits the first writes (or only the TLS ClientHello) into random-length
// pieces written separately with a pause between them, so DPI cannot read them as one segment.
type wraithFragmentConn struct {
	net.Conn
	fragment *wraithFragment
	writes   int64
	done     bool
}

func (c *wraithFragmentConn) Write(b []byte) (int, error) {
	if c.done {
		return c.Conn.Write(b)
	}
	c.writes++
	f := c.fragment
	if f.tlsHello {
		c.done = true
		// TLS handshake record carrying a ClientHello: 0x16, version, length, then type 0x01.
		if len(b) > 5 && b[0] == 0x16 && b[5] == 0x01 {
			recordLen := int(binary.BigEndian.Uint16(b[3:5]))
			if 5+recordLen <= len(b) {
				if err := c.writeHello(b[:5+recordLen]); err != nil {
					return 0, err
				}
				if len(b) > 5+recordLen {
					if _, err := c.Conn.Write(b[5+recordLen:]); err != nil {
						return 0, err
					}
				}
				return len(b), nil
			}
		}
		return c.Conn.Write(b)
	}
	if c.writes > f.last {
		c.done = true
		return c.Conn.Write(b)
	}
	if c.writes < f.first {
		return c.Conn.Write(b)
	}
	if err := c.writeSplit(b); err != nil {
		return 0, err
	}
	return len(b), nil
}

// writeHello re-frames the ClientHello payload into several TLS records (as Xray's tlshello does)
// and sends each record as its own write.
func (c *wraithFragmentConn) writeHello(record []byte) error {
	header, payload := record[:3], record[5:]
	limit := c.fragment.maxSplit.pick()
	var pieces [][]byte
	for len(payload) > 0 {
		size := int(c.fragment.length.pick())
		if size > len(payload) || (limit > 0 && int64(len(pieces))+1 >= limit) {
			size = len(payload)
		}
		piece := make([]byte, 5+size)
		copy(piece, header)
		binary.BigEndian.PutUint16(piece[3:5], uint16(size))
		copy(piece[5:], payload[:size])
		pieces = append(pieces, piece)
		payload = payload[size:]
	}
	return c.writePieces(pieces)
}

func (c *wraithFragmentConn) writeSplit(b []byte) error {
	limit := c.fragment.maxSplit.pick()
	var pieces [][]byte
	for len(b) > 0 {
		size := int(c.fragment.length.pick())
		if size > len(b) || (limit > 0 && int64(len(pieces))+1 >= limit) {
			size = len(b)
		}
		pieces = append(pieces, b[:size])
		b = b[size:]
	}
	return c.writePieces(pieces)
}

func (c *wraithFragmentConn) writePieces(pieces [][]byte) error {
	for i, piece := range pieces {
		if _, err := c.Conn.Write(piece); err != nil {
			return err
		}
		if i < len(pieces)-1 {
			if pause := c.fragment.interval.pick(); pause > 0 {
				time.Sleep(time.Duration(pause) * time.Millisecond)
			}
		}
	}
	return nil
}

func (c *wraithFragmentConn) Upstream() any { return c.Conn }

func sendWraithNoise(noises []wraithNoise, write func([]byte) error) error {
	for _, n := range noises {
		if err := write(n.generate()); err != nil {
			return err
		}
		if pause := n.delay.pick(); pause > 0 {
			time.Sleep(time.Duration(pause) * time.Millisecond)
		}
	}
	return nil
}

// wraithNoiseConn: connected UDP socket; noise goes out once, before the first real datagram.
type wraithNoiseConn struct {
	net.Conn
	noises []wraithNoise
	once   sync.Once
}

func (c *wraithNoiseConn) Write(b []byte) (int, error) {
	var noiseErr error
	c.once.Do(func() {
		noiseErr = sendWraithNoise(c.noises, func(p []byte) error { _, err := c.Conn.Write(p); return err })
	})
	if noiseErr != nil {
		return 0, noiseErr
	}
	return c.Conn.Write(b)
}

func (c *wraithNoiseConn) Upstream() any { return c.Conn }

// wraithNoisePacketConn: unconnected UDP socket (QUIC, Hysteria); noise once per destination.
type wraithNoisePacketConn struct {
	net.PacketConn
	noises []wraithNoise
	access sync.Mutex
	sent   map[string]bool
}

func (c *wraithNoisePacketConn) WriteTo(b []byte, addr net.Addr) (int, error) {
	key := addr.String()
	c.access.Lock()
	first := !c.sent[key]
	c.sent[key] = true
	c.access.Unlock()
	if first {
		if err := sendWraithNoise(c.noises, func(p []byte) error { _, err := c.PacketConn.WriteTo(p, addr); return err }); err != nil {
			return 0, err
		}
	}
	return c.PacketConn.WriteTo(b, addr)
}

func (c *wraithNoisePacketConn) Upstream() any { return c.PacketConn }
