//go:build windows

package main

import "net/http"

func (a *API) handleTerminal(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusNotImplemented, map[string]string{"error": "当前 PTY 终端支持 Linux/macOS，Windows 尚未接入 ConPTY"})
}
