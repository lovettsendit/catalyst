// Command catalyst_check verifies a Catalyst export with the Go somebody
// already has.
//
// It is the sibling of `R/catalyst.R`, and section 11 of docs/interface.md is
// its contract:
//
//	cd go/catalyst
//	go run . -export ../../export/spring
//
// It reads the export's own declaration of its function, recomputes the value
// and every partial derivative over every fixture case, compares within the
// tolerance the export itself declares, and prints how many cases it checked.
// A case that disagrees stops it with a non-zero exit and a message naming
// that case.
//
// Standard library only, and nothing here can start a process, open a socket
// or reach past the directory it was pointed at.
package main

import (
	"flag"
	"fmt"
	"os"
)

func main() {
	export := flag.String("export", "", "the export directory to check")
	flag.Parse()
	if *export == "" {
		fmt.Fprintln(os.Stderr, "usage: catalyst_check -export DIR")
		os.Exit(2)
	}

	checked, err := CheckExport(*export)
	if err != nil {
		fmt.Fprintf(os.Stderr, "catalyst_check: %v\n", err)
		os.Exit(1)
	}
	// A count rather than a bare pass: "it passed" and "it checked nothing"
	// must not read the same.
	fmt.Printf("checked %d cases in %s\n", checked, *export)
}
