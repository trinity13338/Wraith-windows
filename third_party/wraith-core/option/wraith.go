package option

// Wraith: Xray-style fragmentation and noise for the connection to the proxy server.
// See common/dialer/wraith_obfs.go.

type WraithFragmentOptions struct {
	// "tlshello" (default) or a range of writes to split, e.g. "1-3".
	Packets string `json:"packets,omitempty"`
	// Piece length range in bytes, e.g. "50-100".
	Length string `json:"length,omitempty"`
	// Pause between pieces in milliseconds, e.g. "10-20".
	Interval string `json:"interval,omitempty"`
	// Maximum number of pieces, e.g. "100-200"; the last piece carries the remainder. Empty: no limit.
	MaxSplit string `json:"max_split,omitempty"`
}

type WraithNoiseOptions struct {
	// array (comma-separated hex bytes), rand, str, base64 or hex.
	Type   string `json:"type,omitempty"`
	Packet string `json:"packet,omitempty"`
	// Pause after the noise packet in milliseconds, e.g. "10-16".
	Delay string `json:"delay,omitempty"`
	// rand only: packet length range and byte value range.
	Rand      string `json:"rand,omitempty"`
	RandRange string `json:"rand_range,omitempty"`
}
