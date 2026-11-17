package libbox

import("context";"net";"net/http";"net/http/httptest";"sync/atomic";"testing";"time")

func TestProbeMethodsAndConnectionReuse(t *testing.T){
 for _,method:=range []string{"GET","HEAD"}{for _,mode:=range []string{"keepalive","double"}{t.Run(method+mode,func(t *testing.T){
 var requests,connections atomic.Int32
 server:=httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter,r *http.Request){if r.Method!=method{t.Errorf("method = %s",r.Method)};requests.Add(1);w.WriteHeader(204)}));defer server.Close()
 dial:=func(ctx context.Context,network,address string)(net.Conn,error){connections.Add(1);return (&net.Dialer{}).DialContext(ctx,network,address)}
 if _,err:=probeHTTP(server.URL,method,mode,1000,dial);err!=nil{t.Fatal(err)}
 want:=int32(1);if mode=="double"{want=2};if connections.Load()!=want||requests.Load()!=2{t.Fatalf("connections=%d requests=%d",connections.Load(),requests.Load())}
 })}}
}
func TestProbeAcceptsHTTPErrorStatusesAsReachable(t *testing.T){for _,status:=range []int{404,503}{t.Run(http.StatusText(status),func(t *testing.T){s:=httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter,r *http.Request){w.WriteHeader(status)}));defer s.Close();if latency,err:=probeHTTP(s.URL,"GET","keepalive",1000,(&net.Dialer{}).DialContext);err!=nil||latency<0{t.Fatalf("response status %d should count as reachable: latency=%d err=%v",status,latency,err)}})}}
func TestProbeBoundsTimeout(t *testing.T){s:=httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter,r *http.Request){time.Sleep(100*time.Millisecond);w.WriteHeader(204)}));defer s.Close();if _,err:=probeHTTP(s.URL,"HEAD","double",10,(&net.Dialer{}).DialContext);err==nil{t.Fatal("expected timeout")}}
