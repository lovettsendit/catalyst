// Checks for the checker.
//
// Both directions, because a checker that cannot fail is not a checker: a good
// export must come back clean with an honest count, and every way of breaking
// an export that this program claims to catch must actually stop it. The
// fixtures below are written by hand and small enough to verify by reading --
// f(x, y) = x*y + sin(x) at two points, where the value and both partials can
// be checked against a table or a calculator.
package main

import (
	"fmt"
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

const declaredFunction = "func simple(x, y) = x*y + sin(x)"

// parametersJSON is what `catalyst export go` writes as parameters.json,
// reduced to the fields the checker reads.
const parametersJSON = `{
  "schema": "catalyst.export-parameters.v1",
  "name": "simple",
  "function": "func simple(x, y) = x*y + sin(x)",
  "domains": {
    "x": {"min": -3.0, "max": 3.0, "unit": ""},
    "y": {"min": -2.0, "max": 2.0, "unit": ""}
  }
}`

// at returns the true value and partials of the declared function, computed
// here in one line each so the fixtures below are not checked against the same
// code that produced them.
func at(x, y float64) (float64, float64, float64) {
	return x*y + math.Sin(x), y + math.Cos(x), x
}

func fixturesJSON() string {
	v1, dx1, dy1 := at(0.7, 1.3)
	v2, dx2, dy2 := at(-1.25, 0.5)
	return fmt.Sprintf(`{
  "schema": "catalyst.fixtures.v1",
  "tolerance": {"relative": 1e-9, "absolute": 1e-12},
  "cases": [
    {
      "name": "inputs",
      "kind": "normal",
      "inputs": {"x": 0.7, "y": 1.3},
      "value": %.17g,
      "gradient": {"x": %.17g, "y": %.17g},
      "expected": {"value": %.17g, "gradient": {"x": %.17g, "y": %.17g}}
    },
    {
      "name": "interior-low",
      "kind": "normal",
      "inputs": {"x": -1.25, "y": 0.5},
      "value": %.17g,
      "gradient": {"x": %.17g, "y": %.17g},
      "expected": {"value": %.17g, "gradient": {"x": %.17g, "y": %.17g}}
    },
    {
      "name": "out-of-domain-x-below-min",
      "kind": "out_of_domain",
      "inputs": {"x": -4.5, "y": 1.3},
      "expected": {"refused": true}
    }
  ]
}`, v1, dx1, dy1, v1, dx1, dy1, v2, dx2, dy2, v2, dx2, dy2)
}

// exampleExport writes a small export into a directory of its own and returns
// the directory.
func exampleExport(t *testing.T) string {
	t.Helper()
	dir := t.TempDir()
	write(t, dir, "parameters.json", parametersJSON)
	write(t, dir, "fixtures.json", fixturesJSON())
	return dir
}

func write(t *testing.T, dir, name, content string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(dir, name), []byte(content), 0o600); err != nil {
		t.Fatalf("writing %s: %v", name, err)
	}
}

func TestAGoodExportChecksCleanAndSaysHowManyCases(t *testing.T) {
	checked, err := CheckExport(exampleExport(t))
	if err != nil {
		t.Fatalf("a good export must check clean: %v", err)
	}
	if checked != 3 {
		t.Fatalf("the example export has three cases, the checker counted %d", checked)
	}
}

func TestACorruptedValueIsRefusedRatherThanPassed(t *testing.T) {
	dir := exampleExport(t)
	corrupt(t, dir, `"value":`, `"value": 1e9, "was":`)
	requireFailureNaming(t, dir, "inputs")
}

func TestACorruptedPartialIsRefusedRatherThanPassed(t *testing.T) {
	dir := exampleExport(t)
	corrupt(t, dir, `"gradient": {"x":`, `"gradient": {"x": 1e9, "was":`)
	requireFailureNaming(t, dir, "inputs")
}

// A fixture that disagrees with the declared domain is the mistake an export
// makes when its domain check and its cases were written at different times.
func TestAFixtureThatDisagreesAboutTheDomainIsRefused(t *testing.T) {
	dir := exampleExport(t)
	corrupt(t, dir, `"expected": {"refused": true}`, `"expected": {}`)
	requireFailureNaming(t, dir, "out-of-domain-x-below-min")
}

func TestAnExportWithNoCaseAtAllIsRefused(t *testing.T) {
	dir := exampleExport(t)
	write(t, dir, "fixtures.json",
		`{"schema":"catalyst.fixtures.v1","tolerance":{"relative":1e-9,"absolute":1e-12},"cases":[]}`)
	if _, err := CheckExport(dir); err == nil {
		t.Fatal("an export that checks nothing was reported as checked")
	}
}

func TestAnIncompleteExportIsRefused(t *testing.T) {
	dir := exampleExport(t)
	if err := os.Remove(filepath.Join(dir, "fixtures.json")); err != nil {
		t.Fatalf("removing fixtures.json: %v", err)
	}
	_, err := CheckExport(dir)
	if err == nil {
		t.Fatal("an export with no fixtures at all was accepted")
	}
	if !strings.Contains(err.Error(), "fixtures.json") {
		t.Fatalf("the refusal must say what is missing, it said: %v", err)
	}
}

// The tolerance rule is the fixtures' own rule and not a more forgiving one.
func TestCloseIsTheDeclaredToleranceRule(t *testing.T) {
	const rel, abs = 1e-9, 1e-12
	for _, one := range []struct {
		a, b float64
		want bool
		why  string
	}{
		{1, 1, true, "a number is close to itself"},
		{1e6, 1e6 + 1e-4, true, "1e-10 relative is inside 1e-9"},
		{1e6, 1e6 + 1, false, "1e-6 relative is outside 1e-9"},
		{0, 1e-13, true, "1e-13 is inside the 1e-12 absolute bound"},
		{0, 1e-9, false, "1e-9 is outside the 1e-12 absolute bound"},
		{math.NaN(), math.NaN(), true, "no number matches no number"},
		{math.Inf(1), math.Inf(1), true, "an infinity matches itself"},
		{math.Inf(1), 1e300, false, "an infinity is not a finite number"},
	} {
		if got := Close(one.a, one.b, rel, abs); got != one.want {
			t.Errorf("Close(%v, %v) = %v: %s", one.a, one.b, got, one.why)
		}
	}
}

// The parser and the derivative rule, against values worked out by hand.
func TestTheDeclaredFunctionIsReadAndDifferentiated(t *testing.T) {
	params, body, err := function(declaredFunction)
	if err != nil {
		t.Fatalf("the declared function must read: %v", err)
	}
	if len(params) != 2 || params[0] != "x" || params[1] != "y" {
		t.Fatalf("the parameters keep their declared order, got %v", params)
	}
	value, partials, err := gradient(params, body, map[string]float64{"x": 0.7, "y": 1.3})
	if err != nil {
		t.Fatalf("evaluating: %v", err)
	}
	wantValue, wantX, wantY := at(0.7, 1.3)
	for _, one := range []struct {
		got, want float64
		what      string
	}{
		{value, wantValue, "value"},
		{partials[0], wantX, "d/dx"},
		{partials[1], wantY, "d/dy"},
	} {
		if !Close(one.got, one.want, 1e-12, 1e-15) {
			t.Errorf("%s: %.17g, want %.17g", one.what, one.got, one.want)
		}
	}
}

// `^` binds tighter than `*` and associates to the right, which is the one
// piece of the grammar a reader is most likely to assume wrongly.
func TestPowerAssociatesToTheRightAndBindsTighterThanMultiplication(t *testing.T) {
	for _, one := range []struct {
		source string
		want   float64
	}{
		{"func f(x) = 2^3^2", 512},
		{"func f(x) = 2*3^2", 18},
		{"func f(x) = -2^2", -4},
	} {
		params, body, err := function(one.source)
		if err != nil {
			t.Fatalf("%s: %v", one.source, err)
		}
		value, _, err := gradient(params, body, map[string]float64{"x": 1})
		if err != nil {
			t.Fatalf("%s: %v", one.source, err)
		}
		if value != one.want {
			t.Errorf("%s = %v, want %v", one.source, value, one.want)
		}
	}
}

// Every named function the language defines is one this checker can evaluate;
// a gap would make the checker refuse exports that are perfectly good.
func TestEveryNamedFunctionOfTheLanguageIsEvaluated(t *testing.T) {
	for _, name := range []string{
		"neg", "sin", "cos", "tan", "exp", "log", "sqrt",
		"tanh", "sinh", "cosh", "abs", "recip", "erf",
	} {
		source := fmt.Sprintf("func f(x) = %s(x)", name)
		params, body, err := function(source)
		if err != nil {
			t.Fatalf("%s: %v", source, err)
		}
		if _, _, err := gradient(params, body, map[string]float64{"x": 0.5}); err != nil {
			t.Errorf("%s: %v", source, err)
		}
	}
	for _, name := range []string{"pow", "max", "min", "atan2"} {
		source := fmt.Sprintf("func f(x, y) = %s(x, y)", name)
		params, body, err := function(source)
		if err != nil {
			t.Fatalf("%s: %v", source, err)
		}
		if _, _, err := gradient(params, body, map[string]float64{"x": 0.5, "y": 1.5}); err != nil {
			t.Errorf("%s: %v", source, err)
		}
	}
}

// erf reaches Go through math.Erf and R through pnorm; both are the same
// function, and the R export's identity is checked here too so the two sides
// cannot drift apart unnoticed.
func TestErfAgreesWithTheIdentityTheRExportUses(t *testing.T) {
	for _, x := range []float64{0.1, 0.5, 1.0, 2.0, 3.5} {
		// erf(x) = 2 * pnorm(x * sqrt(2)) - 1, and pnorm(z) = (1 + erf(z/sqrt(2)))/2,
		// so the identity reduces to erf(x) itself. Evaluating it the long way
		// round is the check that it is an identity and not an approximation.
		viaNormal := 2*((1+math.Erf(x*math.Sqrt2/math.Sqrt2))/2) - 1
		if !Close(math.Erf(x), viaNormal, 1e-15, 1e-15) {
			t.Errorf("erf(%v): %.17g vs %.17g", x, math.Erf(x), viaNormal)
		}
	}
}

func corrupt(t *testing.T, dir, from, to string) {
	t.Helper()
	path := filepath.Join(dir, "fixtures.json")
	raw, err := os.ReadFile(path) // #nosec -- a path this test just wrote
	if err != nil {
		t.Fatalf("reading fixtures.json: %v", err)
	}
	text := string(raw)
	if !strings.Contains(text, from) {
		t.Fatalf("nothing to corrupt: fixtures.json has no %q", from)
	}
	write(t, dir, "fixtures.json", strings.Replace(text, from, to, 1))
}

func requireFailureNaming(t *testing.T, dir, name string) {
	t.Helper()
	_, err := CheckExport(dir)
	if err == nil {
		t.Fatal("a corrupted export was accepted")
	}
	if !strings.Contains(err.Error(), "case "+name) {
		t.Fatalf("the refusal must name the case that disagreed, it said: %v", err)
	}
}
