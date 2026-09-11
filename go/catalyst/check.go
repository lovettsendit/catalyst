// Checking an export directory: the sibling of `catalyst_check_export` in
// `R/catalyst.R`, holding itself to the same rules.
//
// The rule that matters most is the one about counting. `CheckExport` returns
// how many cases it checked, and the command prints that number, because "it
// passed" and "it checked nothing" must not look the same: an export whose
// fixtures went missing, or whose case list is empty, would otherwise read as
// a clean run. Every disagreement stops the check and names the case, because
// a checker that reports a total without saying which case failed leaves the
// reader exactly where they started.
package main

import (
	"encoding/json"
	"fmt"
	"math"
	"os"
	"path/filepath"
)

// The tolerance a fixture set declares, and the cases it declares it for.
type fixtureFile struct {
	Tolerance struct {
		Relative float64 `json:"relative"`
		Absolute float64 `json:"absolute"`
	} `json:"tolerance"`
	Cases []fixtureCase `json:"cases"`
}

// One case.
//
// `Value` and `Gradient` are the engine's own measurement at the point, when
// it had finite numbers there -- including at an out-of-domain point, where
// they are exactly what a conforming export must refuse to return. `Expected`
// is what the export is required to *do*. Both are checked: they are two
// different claims, and a corruption of either is a corruption.
type fixtureCase struct {
	Name     string             `json:"name"`
	Kind     string             `json:"kind"`
	Inputs   map[string]float64 `json:"inputs"`
	Value    *float64           `json:"value"`
	Gradient map[string]float64 `json:"gradient"`
	Expected struct {
		Value     *float64           `json:"value"`
		Gradient  map[string]float64 `json:"gradient"`
		NonFinite bool               `json:"nonfinite"`
		Refused   bool               `json:"refused"`
	} `json:"expected"`
}

type domain struct {
	Min  float64 `json:"min"`
	Max  float64 `json:"max"`
	Unit string  `json:"unit"`
}

type parameterFile struct {
	Name     string            `json:"name"`
	Function string            `json:"function"`
	Domains  map[string]domain `json:"domains"`
}

// Close is the comparison a fixture declares: within `abs`, or within `rel` of
// the larger magnitude, whichever is more forgiving. The same rule
// `catalyst_close` applies on the R side and `catalyst::export::close` applies
// in the engine, so an export cannot pass one check and fail another.
func Close(a, b, rel, abs float64) bool {
	if math.IsNaN(a) && math.IsNaN(b) {
		return true
	}
	if math.IsInf(a, 0) || math.IsInf(b, 0) || math.IsNaN(a) || math.IsNaN(b) {
		return a == b
	}
	allowed := math.Max(abs, rel*math.Max(math.Abs(a), math.Abs(b)))
	return math.Abs(a-b) <= allowed
}

func readJSON(path string, into interface{}) error {
	raw, err := os.ReadFile(path) // #nosec -- a path the operator named
	if err != nil {
		return fmt.Errorf("%s could not be read: %w", filepath.Base(path), err)
	}
	if err := json.Unmarshal(raw, into); err != nil {
		return fmt.Errorf("%s is not the expected JSON: %w", filepath.Base(path), err)
	}
	return nil
}

// CheckExport reads an export directory, recomputes its function over every
// fixture case, and returns how many cases it checked.
//
// It stops at the first disagreement, naming the case.
func CheckExport(dir string) (int, error) {
	var declared parameterFile
	if err := readJSON(filepath.Join(dir, "parameters.json"), &declared); err != nil {
		return 0, err
	}
	var fixtures fixtureFile
	if err := readJSON(filepath.Join(dir, "fixtures.json"), &fixtures); err != nil {
		return 0, err
	}
	if len(fixtures.Cases) == 0 {
		return 0, fmt.Errorf("the export declares no fixture case, so there is nothing to check")
	}
	rel, abs := fixtures.Tolerance.Relative, fixtures.Tolerance.Absolute
	if rel <= 0 && abs <= 0 {
		return 0, fmt.Errorf("the export declares no tolerance to compare within")
	}

	params, body, err := function(declared.Function)
	if err != nil {
		return 0, fmt.Errorf("the declared function does not read: %w", err)
	}

	checked := 0
	for _, one := range fixtures.Cases {
		value, partials, err := gradient(params, body, one.Inputs)
		if err != nil {
			return checked, fmt.Errorf("case %s: %w", one.Name, err)
		}
		finite := !math.IsNaN(value) && !math.IsInf(value, 0)
		for _, partial := range partials {
			if math.IsNaN(partial) || math.IsInf(partial, 0) {
				finite = false
			}
		}

		// What the export must do here.
		if one.Expected.Refused {
			if inside, why := inDomain(declared, params, one.Inputs); inside {
				return checked, fmt.Errorf(
					"case %s: the fixture says this point is refused, and every input is inside its declared range (%s)",
					one.Name, why)
			}
		} else if inside, why := inDomain(declared, params, one.Inputs); !inside {
			return checked, fmt.Errorf(
				"case %s: the fixture does not say this point is refused, but %s",
				one.Name, why)
		}
		if one.Expected.NonFinite && finite {
			return checked, fmt.Errorf(
				"case %s: the fixture says the function has no finite value here, and it has %.17g",
				one.Name, value)
		}

		// What the engine measured here, and what the export expects. Both
		// are numbers the export is asserting; both have to hold.
		for label, want := range map[string]*float64{
			"value":          one.Value,
			"expected.value": one.Expected.Value,
		} {
			if want == nil {
				continue
			}
			if !Close(value, *want, rel, abs) {
				return checked, fmt.Errorf(
					"case %s: this recomputes %s as %.17g where the export declares %.17g",
					one.Name, label, value, *want)
			}
		}
		for label, want := range map[string]map[string]float64{
			"gradient":          one.Gradient,
			"expected.gradient": one.Expected.Gradient,
		} {
			for i, name := range params {
				declaredPartial, ok := want[name]
				if !ok {
					continue
				}
				if !Close(partials[i], declaredPartial, rel, abs) {
					return checked, fmt.Errorf(
						"case %s: this recomputes %s d/d%s as %.17g where the export declares %.17g",
						one.Name, label, name, partials[i], declaredPartial)
				}
			}
		}
		checked++
	}
	return checked, nil
}

// inDomain reports whether every input lies inside the range the export
// declares for it, and names the first one that does not.
func inDomain(declared parameterFile, params []string, at map[string]float64) (bool, string) {
	for _, name := range params {
		bounds, ok := declared.Domains[name]
		if !ok {
			continue
		}
		coordinate, given := at[name]
		if !given {
			return false, fmt.Sprintf("%s was given no value", name)
		}
		if coordinate < bounds.Min {
			return false, fmt.Sprintf("%s is below its declared minimum", name)
		}
		if coordinate > bounds.Max {
			return false, fmt.Sprintf("%s is above its declared maximum", name)
		}
	}
	return true, ""
}
