package libbox

import (
 "context"
 "fmt"
 "io"
 "net"
 "net/http"
 "time"
)

func probeHTTP(link,method,mode string,timeoutMillis int32,dial func(context.Context,string,string)(net.Conn,error))(int64,error){
 if method!="GET"&&method!="HEAD" {return 0,fmt.Errorf("invalid method")}
 if mode!="keepalive"&&mode!="double" {return 0,fmt.Errorf("invalid mode")}
 if timeoutMillis<1||timeoutMillis>30000 {return 0,fmt.Errorf("invalid timeout")}
 transport:=&http.Transport{DisableKeepAlives:mode!="keepalive",DialContext:dial}
 defer transport.CloseIdleConnections()
 client:=&http.Client{Transport:transport,Timeout:time.Duration(timeoutMillis)*time.Millisecond,CheckRedirect:func(req *http.Request,via []*http.Request)error{return http.ErrUseLastResponse}}
 best:=int64(-1)
 var lastErr error
 for i:=0;i<2;i++ {
  req,err:=http.NewRequest(method,link,nil);if err!=nil{return 0,err}
  start:=time.Now();resp,err:=client.Do(req)
  if err==nil {_,err=io.Copy(io.Discard,io.LimitReader(resp.Body,1024*1024));resp.Body.Close()}
  elapsed:=time.Since(start).Milliseconds()
  if err!=nil {lastErr=err;continue};if best<0||elapsed<best{best=elapsed}
 }
 if best<0{return 0,lastErr};return best,nil
}
