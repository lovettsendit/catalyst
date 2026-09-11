// The expression language an export declares its function in, read and
// evaluated here from scratch.
//
// Why from scratch, rather than calling the export's own generated Go? Because
// an independent check that runs the thing it is checking is not a check. The
// generated `function.go` is the artefact under suspicion; recomputing from
// the source the export itself declares in `parameters.json`, with a parser
// and a derivative rule written here, is the only way for a disagreement to
// mean anything. It is also why this package imports nothing that can start a
// process: there is nothing to start.
//
// The grammar is the whole of `docs/interface.md` section 1:
//
//	func name(a, b) = expression
//
// with `+ - * / ^`, parentheses, unary minus, and the named functions below.
// `^` binds tighter than `*` and associates to the right, so `2^3^2` is 512,
// which is what every notation a reader is likely to be copying from agrees
// on.
package main

import (
	"fmt"
	"math"
	"strconv"
	"strings"
)

// A value and one directional derivative of it, carried together.
//
// Forward mode, one pass per parameter. The export's own gradient comes from a
// single reverse pass, so the two arrive at the same partials by different
// routes -- which is the point. An agreement between two spellings of one
// algorithm would prove much less.
type dual struct {
	v float64
	d float64
}

func constant(x float64) dual { return dual{v: x} }

// ---------------------------------------------------------------------------
// Tokens
// ---------------------------------------------------------------------------

type token struct {
	kind string // "num", "name", "punct", "end"
	text string
	num  float64
}

func lex(source string) ([]token, error) {
	var out []token
	runes := []rune(source)
	i := 0
	for i < len(runes) {
		c := runes[i]
		switch {
		case c == ' ' || c == '\t' || c == '\n' || c == '\r':
			i++
		case c >= '0' && c <= '9' || c == '.':
			start := i
			for i < len(runes) && (runes[i] >= '0' && runes[i] <= '9' || runes[i] == '.') {
				i++
			}
			// An exponent, and the sign that belongs to it rather than to the
			// expression around it.
			if i < len(runes) && (runes[i] == 'e' || runes[i] == 'E') {
				j := i + 1
				if j < len(runes) && (runes[j] == '+' || runes[j] == '-') {
					j++
				}
				if j < len(runes) && runes[j] >= '0' && runes[j] <= '9' {
					for j < len(runes) && runes[j] >= '0' && runes[j] <= '9' {
						j++
					}
					i = j
				}
			}
			text := string(runes[start:i])
			value, err := strconv.ParseFloat(text, 64)
			if err != nil {
				return nil, fmt.Errorf("at byte %d: %q is not a number", start, text)
			}
			out = append(out, token{kind: "num", text: text, num: value})
		case c == '_' || c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z':
			start := i
			for i < len(runes) && (runes[i] == '_' ||
				runes[i] >= 'a' && runes[i] <= 'z' ||
				runes[i] >= 'A' && runes[i] <= 'Z' ||
				runes[i] >= '0' && runes[i] <= '9') {
				i++
			}
			out = append(out, token{kind: "name", text: string(runes[start:i])})
		case strings.ContainsRune("()+-*/^,=", c):
			out = append(out, token{kind: "punct", text: string(c)})
			i++
		default:
			return nil, fmt.Errorf("at byte %d: %q is not part of the language", i, string(c))
		}
	}
	return append(out, token{kind: "end"}), nil
}

// ---------------------------------------------------------------------------
// The tree
// ---------------------------------------------------------------------------

type node struct {
	kind string // "num", "var", "call"
	num  float64
	name string
	args []*node
}

type parser struct {
	tokens []token
	at     int
}

func (p *parser) peek() token { return p.tokens[p.at] }

func (p *parser) take() token {
	t := p.tokens[p.at]
	if t.kind != "end" {
		p.at++
	}
	return t
}

func (p *parser) punct(text string) bool {
	t := p.peek()
	if t.kind == "punct" && t.text == text {
		p.at++
		return true
	}
	return false
}

func (p *parser) expect(text string) error {
	if !p.punct(text) {
		return fmt.Errorf("expected %q, found %q", text, p.peek().text)
	}
	return nil
}

// function is `func name(a, b) = expression`, and returns the parameter names
// in the order they were written, because that order is the gradient's order.
func function(source string) ([]string, *node, error) {
	tokens, err := lex(source)
	if err != nil {
		return nil, nil, err
	}
	p := &parser{tokens: tokens}
	head := p.take()
	if head.kind != "name" || head.text != "func" {
		return nil, nil, fmt.Errorf("a function starts with `func`, this starts with %q", head.text)
	}
	if name := p.take(); name.kind != "name" {
		return nil, nil, fmt.Errorf("`func` is followed by a name, not %q", name.text)
	}
	if err := p.expect("("); err != nil {
		return nil, nil, err
	}
	var params []string
	if !p.punct(")") {
		for {
			t := p.take()
			if t.kind != "name" {
				return nil, nil, fmt.Errorf("a parameter is a name, not %q", t.text)
			}
			params = append(params, t.text)
			if p.punct(",") {
				continue
			}
			break
		}
		if err := p.expect(")"); err != nil {
			return nil, nil, err
		}
	}
	if err := p.expect("="); err != nil {
		return nil, nil, err
	}
	body, err := p.expr()
	if err != nil {
		return nil, nil, err
	}
	if p.peek().kind != "end" {
		return nil, nil, fmt.Errorf("trailing %q after the expression", p.peek().text)
	}
	return params, body, nil
}

func (p *parser) expr() (*node, error) {
	left, err := p.term()
	if err != nil {
		return nil, err
	}
	for {
		t := p.peek()
		if t.kind != "punct" || (t.text != "+" && t.text != "-") {
			return left, nil
		}
		p.at++
		right, err := p.term()
		if err != nil {
			return nil, err
		}
		left = &node{kind: "call", name: t.text, args: []*node{left, right}}
	}
}

func (p *parser) term() (*node, error) {
	left, err := p.unary()
	if err != nil {
		return nil, err
	}
	for {
		t := p.peek()
		if t.kind != "punct" || (t.text != "*" && t.text != "/") {
			return left, nil
		}
		p.at++
		right, err := p.unary()
		if err != nil {
			return nil, err
		}
		left = &node{kind: "call", name: t.text, args: []*node{left, right}}
	}
}

func (p *parser) unary() (*node, error) {
	if p.punct("-") {
		inner, err := p.unary()
		if err != nil {
			return nil, err
		}
		return &node{kind: "call", name: "neg", args: []*node{inner}}, nil
	}
	return p.power()
}

// `^` associates to the right, and its exponent may itself be a unary
// expression, so `2^-1` reads.
func (p *parser) power() (*node, error) {
	base, err := p.atom()
	if err != nil {
		return nil, err
	}
	if p.punct("^") {
		exponent, err := p.unary()
		if err != nil {
			return nil, err
		}
		return &node{kind: "call", name: "pow", args: []*node{base, exponent}}, nil
	}
	return base, nil
}

func (p *parser) atom() (*node, error) {
	t := p.take()
	switch {
	case t.kind == "num":
		return &node{kind: "num", num: t.num}, nil
	case t.kind == "punct" && t.text == "(":
		inner, err := p.expr()
		if err != nil {
			return nil, err
		}
		return inner, p.expect(")")
	case t.kind == "name":
		if !p.punct("(") {
			return &node{kind: "var", name: t.text}, nil
		}
		var args []*node
		if !p.punct(")") {
			for {
				arg, err := p.expr()
				if err != nil {
					return nil, err
				}
				args = append(args, arg)
				if p.punct(",") {
					continue
				}
				break
			}
			if err := p.expect(")"); err != nil {
				return nil, err
			}
		}
		return &node{kind: "call", name: t.text, args: args}, nil
	}
	return nil, fmt.Errorf("expected a number, a name, or `(`, found %q", t.text)
}

// ---------------------------------------------------------------------------
// Evaluation
// ---------------------------------------------------------------------------

func (n *node) eval(env map[string]dual) (dual, error) {
	switch n.kind {
	case "num":
		return constant(n.num), nil
	case "var":
		value, ok := env[n.name]
		if !ok {
			return dual{}, fmt.Errorf("%q is not a parameter of this function", n.name)
		}
		return value, nil
	}
	args := make([]dual, len(n.args))
	for i, arg := range n.args {
		value, err := arg.eval(env)
		if err != nil {
			return dual{}, err
		}
		args[i] = value
	}
	return apply(n.name, args)
}

func apply(name string, args []dual) (dual, error) {
	if len(args) == 2 {
		a, b := args[0], args[1]
		switch name {
		case "+":
			return dual{a.v + b.v, a.d + b.d}, nil
		case "-":
			return dual{a.v - b.v, a.d - b.d}, nil
		case "*":
			return dual{a.v * b.v, a.d*b.v + a.v*b.d}, nil
		case "/":
			return dual{a.v / b.v, (a.d*b.v - a.v*b.d) / (b.v * b.v)}, nil
		case "pow":
			return power(a, b), nil
		// The engine's rule, not Go's: `a >= b ? a : b`, and the derivative
		// belongs to whichever side was taken.
		case "max":
			if a.v >= b.v {
				return a, nil
			}
			return b, nil
		case "min":
			if a.v <= b.v {
				return a, nil
			}
			return b, nil
		case "atan2":
			denominator := a.v*a.v + b.v*b.v
			return dual{math.Atan2(a.v, b.v), (a.d*b.v - a.v*b.d) / denominator}, nil
		}
		return dual{}, fmt.Errorf("%q is not a function of two arguments", name)
	}
	if len(args) != 1 {
		return dual{}, fmt.Errorf("%q was given %d arguments", name, len(args))
	}
	x := args[0]
	// value, and d/dx of it; the chain rule is applied once, below.
	var v, slope float64
	switch name {
	case "neg":
		v, slope = -x.v, -1
	case "sin":
		v, slope = math.Sin(x.v), math.Cos(x.v)
	case "cos":
		v, slope = math.Cos(x.v), -math.Sin(x.v)
	case "tan":
		c := math.Cos(x.v)
		v, slope = math.Tan(x.v), 1/(c*c)
	case "exp":
		v = math.Exp(x.v)
		slope = v
	case "log":
		v, slope = math.Log(x.v), 1/x.v
	case "sqrt":
		v = math.Sqrt(x.v)
		slope = 0.5 / v
	case "tanh":
		v = math.Tanh(x.v)
		slope = 1 - v*v
	case "sinh":
		v, slope = math.Sinh(x.v), math.Cosh(x.v)
	case "cosh":
		v, slope = math.Cosh(x.v), math.Sinh(x.v)
	case "abs":
		v = math.Abs(x.v)
		slope = 1
		if x.v < 0 {
			slope = -1
		}
	case "recip":
		v, slope = 1/x.v, -1/(x.v*x.v)
	case "erf":
		v = math.Erf(x.v)
		slope = 2 / math.Sqrt(math.Pi) * math.Exp(0-x.v*x.v)
	default:
		return dual{}, fmt.Errorf("%q is not a function this language defines", name)
	}
	return dual{v, slope * x.d}, nil
}

// power handles the two cases separately because the general rule goes through
// a logarithm, and a constant exponent -- which is nearly every exponent
// anybody writes -- has a derivative that is defined for a negative base where
// the general one is not.
func power(a, b dual) dual {
	v := math.Pow(a.v, b.v)
	if b.d == 0 {
		return dual{v, b.v * math.Pow(a.v, b.v-1) * a.d}
	}
	return dual{v, v * (b.d*math.Log(a.v) + b.v*a.d/a.v)}
}

// gradient evaluates the function at a point and returns the value and one
// partial derivative per parameter, in declaration order.
func gradient(params []string, body *node, at map[string]float64) (float64, []float64, error) {
	partials := make([]float64, len(params))
	if len(params) == 0 {
		out, err := body.eval(map[string]dual{})
		return out.v, partials, err
	}
	value := math.NaN()
	for i, seeded := range params {
		env := make(map[string]dual, len(params))
		for _, name := range params {
			coordinate, ok := at[name]
			if !ok {
				return 0, nil, fmt.Errorf("no value was given for %q", name)
			}
			derivative := 0.0
			if name == seeded {
				derivative = 1
			}
			env[name] = dual{coordinate, derivative}
		}
		out, err := body.eval(env)
		if err != nil {
			return 0, nil, err
		}
		value = out.v
		partials[i] = out.d
	}
	return value, partials, nil
}
