package dialer

import (
	"bytes"
	"context"
	"encoding/binary"
	"io"
	"net"
	"testing"
	"time"

	"github.com/sagernet/sing-box/option"
	M "github.com/sagernet/sing/common/metadata"
	N "github.com/sagernet/sing/common/network"
)

type plainDialer struct{}

func (plainDialer) DialContext(ctx context.Context, network string, destination M.Socksaddr) (net.Conn, error) {
	var d net.Dialer
	return d.DialContext(ctx, network, destination.String())
}

func (plainDialer) ListenPacket(ctx context.Context, destination M.Socksaddr) (net.PacketConn, error) {
	return net.ListenPacket("udp", "127.0.0.1:0")
}

func clientHello(payloadLen int) []byte {
	payload := make([]byte, payloadLen)
	payload[0] = 0x01 // handshake type: ClientHello
	for i := 1; i < len(payload); i++ {
		payload[i] = byte(i)
	}
	record := []byte{0x16, 0x03, 0x01, 0, 0}
	binary.BigEndian.PutUint16(record[3:5], uint16(payloadLen))
	return append(record, payload...)
}

// The server must reassemble exactly the original ClientHello payload from the re-framed records.
func TestWraithTLSHelloReframing(t *testing.T) {
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	received := make(chan []byte, 1)
	go func() {
		conn, err := listener.Accept()
		if err != nil {
			return
		}
		defer conn.Close()
		data, _ := io.ReadAll(conn)
		received <- data
	}()
	d, err := wrapWraithObfs(plainDialer{}, option.DialerOptions{AbstractDialerOptions: option.AbstractDialerOptions{
		WraithFragment: &option.WraithFragmentOptions{Packets: "tlshello", Length: "10-30", Interval: "1-2"},
	}})
	if err != nil {
		t.Fatal(err)
	}
	conn, err := d.DialContext(context.Background(), N.NetworkTCP, M.ParseSocksaddr(listener.Addr().String()))
	if err != nil {
		t.Fatal(err)
	}
	hello := clientHello(300)
	if _, err = conn.Write(append(hello, []byte("tail")...)); err != nil {
		t.Fatal(err)
	}
	conn.Close()
	data := <-received
	var payload []byte
	var sizes []int
	for len(data) >= 5 && data[0] == 0x16 {
		size := int(binary.BigEndian.Uint16(data[3:5]))
		sizes = append(sizes, size)
		payload = append(payload, data[5:5+size]...)
		data = data[5+size:]
	}
	records := len(sizes)
	// Every record is 10–30 bytes; only the last one may be shorter (the remainder).
	for i, size := range sizes {
		if size > 30 || (size < 10 && i != len(sizes)-1) {
			t.Fatalf("record %d has %d bytes, want 10-30: %v", i, size, sizes)
		}
	}
	if records < 10 {
		t.Fatalf("expected the ClientHello split into many records, got %d", records)
	}
	if !bytes.Equal(payload, hello[5:]) {
		t.Fatal("reassembled ClientHello payload differs from the original")
	}
	if string(data) != "tail" {
		t.Fatalf("data after the ClientHello was changed: %q", data)
	}
}

// Write-range mode: the first writes are split into pieces, later ones pass untouched,
// and the byte stream is unchanged.
func TestWraithWriteRangeSplit(t *testing.T) {
	server, client := net.Pipe()
	f, err := buildWraithFragment(&option.WraithFragmentOptions{Packets: "1-1", Length: "3-5", Interval: "0", MaxSplit: "4"})
	if err != nil {
		t.Fatal(err)
	}
	conn := &wraithFragmentConn{Conn: client, fragment: f}
	var chunks []int
	done := make(chan []byte)
	go func() {
		var all []byte
		buf := make([]byte, 1024)
		for {
			n, err := server.Read(buf)
			if n > 0 {
				chunks = append(chunks, n)
				all = append(all, buf[:n]...)
			}
			if err != nil {
				done <- all
				return
			}
		}
	}()
	first := bytes.Repeat([]byte("a"), 40)
	second := bytes.Repeat([]byte("b"), 40)
	conn.Write(first)
	conn.Write(second)
	client.Close()
	all := <-done
	if !bytes.Equal(all, append(first, second...)) {
		t.Fatal("stream changed by fragmentation")
	}
	// max_split 4: the first write goes out in exactly 4 pieces, the second in one.
	if len(chunks) != 5 {
		t.Fatalf("got %d writes %v, want 4 pieces + 1 untouched write", len(chunks), chunks)
	}
	if chunks[4] != 40 {
		t.Fatalf("second write was split: %v", chunks)
	}
}

// Noise must reach the destination before the first real datagram, once per destination.
func TestWraithNoiseBeforeFirstDatagram(t *testing.T) {
	server, err := net.ListenPacket("udp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer server.Close()
	d, err := wrapWraithObfs(plainDialer{}, option.DialerOptions{AbstractDialerOptions: option.AbstractDialerOptions{
		WraithNoise: []option.WraithNoiseOptions{
			{Type: "array", Packet: "1A,2B,3C", Delay: "1-2"},
			{Type: "rand", Rand: "5-8", RandRange: "0-255", Delay: "0"},
		},
	}})
	if err != nil {
		t.Fatal(err)
	}
	conn, err := d.ListenPacket(context.Background(), M.ParseSocksaddr(server.LocalAddr().String()))
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	conn.WriteTo([]byte("real-1"), server.LocalAddr())
	conn.WriteTo([]byte("real-2"), server.LocalAddr())
	server.SetReadDeadline(time.Now().Add(2 * time.Second))
	var got [][]byte
	buf := make([]byte, 2048)
	for i := 0; i < 4; i++ {
		n, _, err := server.ReadFrom(buf)
		if err != nil {
			t.Fatal(err)
		}
		got = append(got, append([]byte(nil), buf[:n]...))
	}
	if !bytes.Equal(got[0], []byte{0x1a, 0x2b, 0x3c}) {
		t.Fatalf("first datagram %x, want array noise 1a2b3c", got[0])
	}
	if len(got[1]) < 5 || len(got[1]) > 8 {
		t.Fatalf("rand noise length %d, want 5-8", len(got[1]))
	}
	if string(got[2]) != "real-1" || string(got[3]) != "real-2" {
		t.Fatalf("real datagrams %q %q, noise must be sent only once", got[2], got[3])
	}
}

func TestWraithOptionsValidation(t *testing.T) {
	if _, err := buildWraithNoises([]option.WraithNoiseOptions{{Type: "array", Packet: "ZZ"}}); err == nil {
		t.Fatal("bad hex byte accepted")
	}
	if _, err := buildWraithFragment(&option.WraithFragmentOptions{Length: "0-5"}); err == nil {
		t.Fatal("zero piece length accepted")
	}
	d, err := wrapWraithObfs(plainDialer{}, option.DialerOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if _, wrapped := d.(*wraithDialer); wrapped {
		t.Fatal("dialer wrapped without any wraith option")
	}
}
