package webui

import (
	"embed"
	"io/fs"
)

//go:embed all:dist
var assets embed.FS

//go:embed icon.png
var Icon []byte

func Assets() fs.FS {
	sub, err := fs.Sub(assets, "dist")
	if err != nil {
		panic(err)
	}
	return sub
}
