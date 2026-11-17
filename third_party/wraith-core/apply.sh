#!/bin/sh
# Applies the Wraith patches to an extracted sing-box 1.14.1 source tree.
# Usage: third_party/wraith-core/apply.sh <path-to-extracted-sing-box>
# The upstream archive uses CRLF line endings, so the edits below accept both.
set -e
here=$(cd "$(dirname "$0")" && pwd)
core=${1:?path to extracted sing-box source}

# Per-outbound GET/HEAD probes and Reality key-share compatibility (see README).
cp "$here"/libbox/*.go "$core/experimental/libbox/"
cp "$here/reality_client.go" "$core/common/tls/reality_client.go"

# Xray-style fragmentation and UDP noise on the dialer to the proxy server.
cp "$here/option/wraith.go" "$core/option/wraith.go"
cp "$here/dialer/wraith_obfs.go" "$core/common/dialer/wraith_obfs.go"
if ! grep -q 'WraithFragment' "$core/option/outbound.go"; then
  perl -0pi -e 's/(type AbstractDialerOptions struct \{(\r?\n))/$1\tWraithFragment *WraithFragmentOptions `json:"wraith_fragment,omitempty" schema:"omit"`$2\tWraithNoise []WraithNoiseOptions `json:"wraith_noise,omitempty" schema:"omit"`$2/' "$core/option/outbound.go"
fi
if ! grep -q 'wrapWraithObfs' "$core/common/dialer/dialer.go"; then
  # The only "return dialer, nil" after the resolve dialer is NewWithOptions' final return.
  perl -0pi -e 's/(resolveFallbackDelay,\r?\n\t\t\)\r?\n\t\}\r?\n\t)return dialer, nil/$1return wrapWraithObfs(dialer, dialOptions)/' "$core/common/dialer/dialer.go"
fi
grep -q 'WraithFragment' "$core/option/outbound.go"
grep -q 'wrapWraithObfs' "$core/common/dialer/dialer.go"
echo "wraith-core patches applied to $core"
