//go:build !windows

package main

import (
	"encoding/json"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/creack/pty"
	"github.com/gorilla/websocket"
	"golang.org/x/sys/unix"
)

func (a *API) handleTerminal(w http.ResponseWriter, r *http.Request) {
	if r.Method != http.MethodGet {
		methodNotAllowed(w)
		return
	}
	path, err := a.authorizedRepo(r.URL.Query().Get("repo"))
	if err != nil {
		writeError(w, err)
		return
	}
	upgrader := websocket.Upgrader{CheckOrigin: func(r *http.Request) bool {
		origin, err := url.Parse(r.Header.Get("Origin"))
		return err == nil && (origin.Scheme == "http" || origin.Scheme == "https") && strings.EqualFold(origin.Host, r.Host)
	}}
	conn, err := upgrader.Upgrade(w, r, nil)
	if err != nil {
		return
	}
	defer conn.Close()
	var writer sync.Mutex
	send := func(kind int, data []byte) error {
		writer.Lock()
		defer writer.Unlock()
		_ = conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
		return conn.WriteMessage(kind, data)
	}
	event := func(value any) error {
		data, _ := json.Marshal(value)
		return send(websocket.TextMessage, data)
	}
	shell := os.Getenv("SHELL")
	if shell == "" {
		shell = "/bin/bash"
	}
	if _, err := exec.LookPath(shell); err != nil {
		shell = "/bin/sh"
	}
	cmd := exec.Command(shell, "-i")
	cmd.Dir = path
	// Keep desktop credentials and the CLI PATH; an interactive shell loads its own rc file.
	for _, entry := range os.Environ() {
		name, value, _ := strings.Cut(entry, "=")
		switch strings.ToLower(name) {
		case "http_proxy", "https_proxy", "all_proxy":
			// A stale PM2 placeholder must not shadow a working proxy setting.
			proxy, err := url.Parse(value)
			if value != "" && (err != nil || proxy.Host == "") {
				continue
			}
		}
		cmd.Env = append(cmd.Env, entry)
	}
	cmd.Env = append(cmd.Env, "TERM=xterm-256color", "COLORTERM=truecolor")
	// Ensure user-installed CLIs (including Codex) are available to PM2-launched shells.
	if home, err := os.UserHomeDir(); err == nil {
		cmd.Env = append(cmd.Env, "PATH="+filepath.Join(home, ".local", "bin")+string(os.PathListSeparator)+os.Getenv("PATH"))
	}
	terminal, err := pty.StartWithSize(cmd, &pty.Winsize{Rows: 24, Cols: 80})
	if err != nil {
		_ = event(map[string]any{"type": "error", "message": err.Error()})
		return
	}
	_ = unix.SetNonblock(int(terminal.Fd()), true)
	done := make(chan struct{})
	var flowMu sync.Mutex
	flow := make(chan struct{})
	close(flow)
	paused := false
	defer func() {
		close(done)
		// Job-control programs have their own foreground process group.
		if foreground, err := unix.IoctlGetInt(int(terminal.Fd()), unix.TIOCGPGRP); err == nil && foreground > 0 {
			_ = unix.Kill(-foreground, unix.SIGHUP)
			_ = unix.Kill(-foreground, unix.SIGKILL)
		}
		_ = unix.Kill(-cmd.Process.Pid, unix.SIGHUP)
		_ = terminal.Close()
		_ = cmd.Process.Kill()
		_ = cmd.Wait()
		_, _ = a.refreshRepository(path)
	}()
	_ = event(map[string]any{"type": "ready", "shell": shell, "path": path})
	go func() {
		buffer := make([]byte, 32*1024)
		for {
			n, err := terminal.Read(buffer)
			if n > 0 {
				flowMu.Lock()
				gate := flow
				flowMu.Unlock()
				select {
				case <-gate:
				case <-done:
					return
				}
				if err := send(websocket.BinaryMessage, buffer[:n]); err != nil {
					break
				}
			}
			if err != nil {
				break
			}
		}
		_ = event(map[string]any{"type": "exit"})
		_ = conn.Close()
	}()
	// Sync changes made by shells, editors and long-running coding assistants.
	go func() {
		ticker := time.NewTicker(3 * time.Second)
		defer ticker.Stop()
		ping := time.NewTicker(20 * time.Second)
		defer ping.Stop()
		var previous string
		for {
			select {
			case <-done:
				return
			case <-ping.C:
				if err := conn.WriteControl(websocket.PingMessage, nil, time.Now().Add(10*time.Second)); err != nil {
					_ = conn.Close()
					return
				}
			case <-ticker.C:
				repo, err := loadRepository(path)
				if err != nil {
					continue
				}
				data, _ := json.Marshal(repo)
				if string(data) == previous {
					continue
				}
				updated, err := a.refreshRepository(path)
				if err != nil {
					continue
				}
				previous = string(data)
				if event(map[string]any{"type": "repo", "repo": updated}) != nil {
					_ = conn.Close()
					return
				}
			}
		}
	}()
	conn.SetReadLimit(64 * 1024)
	_ = conn.SetReadDeadline(time.Now().Add(75 * time.Second))
	conn.SetPongHandler(func(string) error { return conn.SetReadDeadline(time.Now().Add(75 * time.Second)) })
	for {
		var message struct {
			Type string `json:"type"`
			Data string `json:"data"`
			Cols uint16 `json:"cols"`
			Rows uint16 `json:"rows"`
		}
		kind, data, err := conn.ReadMessage()
		if err != nil {
			return
		}
		if kind == websocket.BinaryMessage {
			if _, err := terminal.Write(data); err != nil {
				return
			}
			continue
		}
		if err := json.Unmarshal(data, &message); err != nil {
			return
		}
		switch message.Type {
		case "pause":
			flowMu.Lock()
			if !paused {
				paused = true
				flow = make(chan struct{})
			}
			flowMu.Unlock()
		case "resume":
			flowMu.Lock()
			if paused {
				paused = false
				close(flow)
			}
			flowMu.Unlock()
		case "input":
			if _, err := terminal.Write([]byte(message.Data)); err != nil {
				return
			}
		case "resize":
			if message.Cols >= 2 && message.Cols <= 1000 && message.Rows >= 2 && message.Rows <= 500 {
				_ = pty.Setsize(terminal, &pty.Winsize{Cols: message.Cols, Rows: message.Rows})
			}
		}
	}
}
