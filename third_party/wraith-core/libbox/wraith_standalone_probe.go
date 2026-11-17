package libbox

import (
 "context"
 "fmt"
 "net"
 box "github.com/sagernet/sing-box"
 "github.com/sagernet/sing-box/adapter"
 "github.com/sagernet/sing-box/option"
 tun "github.com/sagernet/sing-tun"
 "github.com/sagernet/sing/common/logger"
 M "github.com/sagernet/sing/common/metadata"
 "github.com/sagernet/sing/service"
)

type ProbeProtector interface { Protect(fd int32) bool }
type probePlatform struct { platformInterfaceStub; protector ProbeProtector }
func (p *probePlatform) AutoDetectInterfaceControl(fd int) error {
 if p.protector!=nil && !p.protector.Protect(int32(fd)){return fmt.Errorf("protect probe socket failed")};return nil
}
func (p *probePlatform) CreateDefaultInterfaceMonitor(_ logger.Logger) tun.DefaultInterfaceMonitor {return &probeMonitor{}}
type probeMonitor struct{interfaceMonitorStub}
func (*probeMonitor) Start()error{return nil}
func (*probeMonitor) Close()error{return nil}

// ProbeConfig creates an isolated outbound-only instance. It never creates a TUN,
// listening socket, command server, cache file or session log.
func ProbeConfig(config, tag, link, method, mode string, timeoutMillis int32, protector ProbeProtector)(int64,error){
 ctx,cancel:=context.WithCancel(baseContext(nil));defer cancel()
 ctx=service.ContextWith[adapter.PlatformInterface](ctx,&probePlatform{protector:protector})
 options,err:=parseConfig(ctx,config);if err!=nil{return 0,err}
 options.Log=&option.LogOptions{Disabled:true}
 options.Inbounds=nil;options.Services=nil;options.Experimental=nil
 instance,err:=box.New(box.Options{Context:ctx,Options:options});if err!=nil{return 0,err}
 if err=instance.Start();err!=nil{return 0,err};defer instance.Close()
 outbound,ok:=instance.Outbound().Outbound(tag);if !ok{return 0,fmt.Errorf("probe outbound is missing")}
 return probeHTTP(link,method,mode,timeoutMillis,func(ctx context.Context,network,address string)(net.Conn,error){return outbound.DialContext(ctx,network,M.ParseSocksaddr(address))})
}
