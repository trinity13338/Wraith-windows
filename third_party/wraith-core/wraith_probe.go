package libbox

import (
 "context"
 "fmt"
 "net"
 M "github.com/sagernet/sing/common/metadata"
)

// ProbeOutbound measures a specific outbound without changing the selected route.
func (s *CommandServer) ProbeOutbound(tag, link, method, mode string, timeoutMillis int32) (int64, error) {
 instance := s.StartedService.Instance()
 if instance == nil || instance.Box() == nil { return 0, fmt.Errorf("VPN is not connected") }
 outbound, ok := instance.Box().Outbound().Outbound(tag)
 if !ok { return 0, fmt.Errorf("server is not loaded; reconnect VPN") }
 return probeHTTP(link,method,mode,timeoutMillis,func(ctx context.Context,network,address string)(net.Conn,error){return outbound.DialContext(ctx,network,M.ParseSocksaddr(address))})
}
