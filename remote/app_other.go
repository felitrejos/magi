//go:build !darwin

package main

import (
	"io"
	"log"
)

func setUpApp(io.Writer) error {
	return nil
}

func updateApp(*log.Logger) {}
