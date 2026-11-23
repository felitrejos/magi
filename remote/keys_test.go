package main

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

func TestTheLiveFileUsesTheKeysTheAppReads(t *testing.T) {
	paths, err := filepath.Glob("../mac/MagiTests/Fixtures/live/*.json")
	if err != nil || len(paths) == 0 {
		t.Fatalf("the app's live fixtures are missing: %v", err)
	}
	for _, path := range paths {
		data, err := os.ReadFile(path)
		if err != nil {
			t.Fatal(err)
		}
		var live liveMachine
		if err := json.Unmarshal(data, &live); err != nil {
			t.Fatal(err)
		}
		written, err := json.Marshal(live)
		if err != nil {
			t.Fatal(err)
		}
		var read, wrote map[string]any
		if err := json.Unmarshal(data, &read); err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(written, &wrote); err != nil {
			t.Fatal(err)
		}
		if !reflect.DeepEqual(read, wrote) {
			t.Errorf("%s reads\n%s\nand the watcher writes\n%s", filepath.Base(path), data, written)
		}
	}
}
