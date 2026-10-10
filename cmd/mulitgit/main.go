package main

import (
	"context"
	"log"
	"mulitgit/internal/webui"
	"mulitgit/internal/workbench"
	"net"
	"net/http"
	"os"
	"runtime"
	"time"
)

func main() {
	api, err := workbench.New("")
	if err != nil {
		log.Fatal(err)
	}
	api.Start(context.Background())
	addr := "127.0.0.1:3210"
	if value := os.Getenv("MULITGIT_ADDR"); value != "" {
		host, _, err := net.SplitHostPort(value)
		if err != nil || (host != "127.0.0.1" && host != "localhost" && host != "::1" && host != "0.0.0.0") {
			log.Fatal("MULITGIT_ADDR must bind to localhost or 0.0.0.0")
		}
		addr = value
	}
	server := &http.Server{Addr: addr, Handler: api.Handler(webui.Assets()), ReadHeaderTimeout: 5 * time.Second}
	log.Printf("MulitGit listening at http://%s (%s)", addr, runtime.GOOS)
	log.Fatal(server.ListenAndServe())
}
