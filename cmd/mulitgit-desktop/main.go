package main

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/hex"
	"fmt"
	"log"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"time"

	"mulitgit/internal/webui"
	"mulitgit/internal/workbench"

	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	"github.com/wailsapp/wails/v2/pkg/options/linux"
	"github.com/wailsapp/wails/v2/pkg/runtime"
)

type Connection struct {
	BaseURL string `json:"baseURL"`
	Token   string `json:"token"`
}

type DesktopApp struct {
	ctx        context.Context
	connection Connection
	server     *http.Server
	cancel     context.CancelFunc
	once       sync.Once
}

// Connection exposes the private backend only through Wails' native binding.
func (a *DesktopApp) Connection() Connection { return a.connection }
func (a *DesktopApp) ChooseDirectory() (string, error) {
	return runtime.OpenDirectoryDialog(a.ctx, runtime.OpenDialogOptions{Title: "选择监控目录", CanCreateDirectories: true})
}
func (a *DesktopApp) startup(ctx context.Context) {
	a.ctx = ctx
	running, err := newDesktopApp()
	if err != nil {
		log.Printf("desktop startup: %v", err)
		_, _ = runtime.MessageDialog(ctx, runtime.MessageDialogOptions{Type: runtime.ErrorDialog, Title: "MulitGit 启动失败", Message: err.Error()})
		runtime.Quit(ctx)
		return
	}
	a.connection, a.server, a.cancel = running.connection, running.server, running.cancel
}
func (a *DesktopApp) shutdown(context.Context) { a.close() }
func (a *DesktopApp) close() {
	a.once.Do(func() {
		if a.cancel == nil || a.server == nil {
			return
		}
		a.cancel()
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()
		if err := a.server.Shutdown(ctx); err != nil {
			_ = a.server.Close()
		}
	})
}

func desktopConfigPath() (string, error) {
	base, err := os.UserConfigDir()
	if err != nil {
		return "", err
	}
	path := filepath.Join(base, "mulitgit-desktop", "config.json")
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return "", err
	}
	// Import Web settings once; separate files avoid two instances overwriting each other.
	if _, err := os.Stat(path); os.IsNotExist(err) {
		if data, readErr := os.ReadFile(workbench.ConfigFilePath()); readErr == nil {
			if err := os.WriteFile(path, data, 0o600); err != nil {
				return "", err
			}
			if snapshot, err := os.ReadFile(workbench.ConfigFilePath() + ".repos.json"); err == nil {
				if err := os.WriteFile(path+".repos.json", snapshot, 0o600); err != nil {
					return "", err
				}
			}
		}
	}
	return path, nil
}

func newDesktopApp() (*DesktopApp, error) {
	path, err := desktopConfigPath()
	if err != nil {
		return nil, err
	}
	api, err := workbench.New(path)
	if err != nil {
		return nil, err
	}
	api.AllowDesktopOrigins = true
	secret := make([]byte, 32)
	if _, err := rand.Read(secret); err != nil {
		return nil, err
	}
	token := hex.EncodeToString(secret)
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		return nil, err
	}
	ctx, cancel := context.WithCancel(context.Background())
	api.Start(ctx)
	handler := api.Handler(webui.Assets())
	server := &http.Server{ReadHeaderTimeout: 5 * time.Second, Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		origin := r.Header.Get("Origin")
		if api.DesktopOrigin(origin) {
			w.Header().Set("Access-Control-Allow-Origin", origin)
			w.Header().Set("Vary", "Origin")
			w.Header().Set("Access-Control-Allow-Headers", "Content-Type, X-MulitGit-Token")
			w.Header().Set("Access-Control-Allow-Methods", "GET, POST, DELETE, OPTIONS")
		} else if origin != "" {
			http.Error(w, "origin denied", http.StatusForbidden)
			return
		}
		if r.Method == http.MethodOptions {
			w.WriteHeader(http.StatusNoContent)
			return
		}
		provided := r.Header.Get("X-MulitGit-Token")
		if provided == "" {
			provided = r.URL.Query().Get("desktopToken")
		}
		if subtle.ConstantTimeCompare([]byte(provided), []byte(token)) != 1 {
			http.Error(w, "unauthorized", http.StatusUnauthorized)
			return
		}
		handler.ServeHTTP(w, r)
	})}
	app := &DesktopApp{connection: Connection{BaseURL: "http://" + listener.Addr().String(), Token: token}, server: server, cancel: cancel}
	go func() {
		if err := server.Serve(listener); err != nil && err != http.ErrServerClosed {
			log.Printf("desktop backend: %v", err)
		}
	}()
	return app, nil
}

func main() {
	app := &DesktopApp{}
	defer app.close()
	err := wails.Run(&options.App{
		Title: "MulitGit", Width: 1280, Height: 820, MinWidth: 960, MinHeight: 600,
		BackgroundColour: &options.RGBA{R: 244, G: 245, B: 247, A: 255},
		AssetServer:      &assetserver.Options{Assets: webui.Assets()},
		OnStartup:        app.startup, OnShutdown: app.shutdown,
		Bind: []interface{}{app},
		SingleInstanceLock: &options.SingleInstanceLock{UniqueId: "mulitgit-wails-desktop", OnSecondInstanceLaunch: func(options.SecondInstanceData) {
			runtime.WindowShow(app.ctx)
			runtime.WindowUnminimise(app.ctx)
		}},
		Linux: &linux.Options{ProgramName: "MulitGit", Icon: webui.Icon},
	})
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
