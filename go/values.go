package termtrace

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"math/big"
	"slices"
	"strconv"
	"unicode/utf8"
)

const (
	decimalBase = 10
	uint64Bits  = 64
	uint128Bits = 128
)

var (
	// decodeOptions decodes every generated union; it is set by the generated init function.
	decodeOptions json.Options

	maxUint128 = new(big.Int).Sub(new(big.Int).Lsh(big.NewInt(1), uint128Bits), big.NewInt(1))

	errUnexpectedJSON = errors.New("termtrace: unexpected JSON")
)

// Path is a filesystem path carried exactly. It may hold bytes that are not UTF-8; those travel
// as base64.
type Path string //nolint:recvcheck // UnmarshalJSONFrom must mutate; values keep MarshalJSONTo.

type pathBase64 struct {
	Base64 []byte `json:"base64"`
}

// DecimalU64 is an unsigned 64-bit integer carried as decimal text.
type DecimalU64 uint64 //nolint:recvcheck // UnmarshalJSONFrom must mutate; values keep MarshalJSONTo.

// Uint128 is an unsigned 128-bit integer carried as a JSON number.
type Uint128 struct { //nolint:recvcheck // UnmarshalJSONFrom must mutate; values keep String and Big.
	// Hi holds the upper 64 bits.
	Hi uint64
	// Lo holds the lower 64 bits.
	Lo uint64
}

// DecimalU128 is an unsigned 128-bit integer carried as decimal text.
type DecimalU128 struct { //nolint:recvcheck // UnmarshalJSONFrom must mutate; values keep MarshalJSONTo.
	Uint128
}

// Nullable is a field where an explicit null means something different from leaving the field
// out. The zero value leaves the field out.
type Nullable[T any] struct {
	value   *T
	present bool
}

// Pair is a two-element JSON array with typed elements.
type Pair[A, B any] struct {
	// First is the first element.
	First A
	// Second is the second element.
	Second B
}

// DecodeJSON decodes a value written by the server or by [json.Marshal], including the
// interfaces that represent unions.
func DecodeJSON(data []byte, v any) error {
	//nolint:wrapcheck // DecodeJSON reports json/v2 errors unchanged, like json.Unmarshal.
	return json.Unmarshal(data, v, decodeOptions)
}

// Null returns a Nullable that sends an explicit null.
func Null[T any]() Nullable[T] {
	return Nullable[T]{value: nil, present: true}
}

// NonNull returns a Nullable that sends value.
func NonNull[T any](value T) Nullable[T] {
	return Nullable[T]{value: &value, present: true}
}

// MarshalJSONTo writes the path as a string, or as base64 when it is not UTF-8.
func (p Path) MarshalJSONTo(enc *jsontext.Encoder) error {
	if utf8.ValidString(string(p)) {
		//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
		return enc.WriteToken(jsontext.String(string(p)))
	}

	//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	return json.MarshalEncode(enc, pathBase64{Base64: []byte(p)})
}

// UnmarshalJSONFrom reads a path written as a string or as base64.
func (p *Path) UnmarshalJSONFrom(dec *jsontext.Decoder) error {
	switch dec.PeekKind() {
	case jsontext.KindString:
		token, err := dec.ReadToken()
		if err != nil {
			return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
		}

		*p = Path(token.String())

		return nil
	case jsontext.KindBeginObject:
		var encoded pathBase64
		if err := json.UnmarshalDecode(dec, &encoded); err != nil {
			return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
		}

		*p = Path(encoded.Base64)

		return nil
	case jsontext.KindInvalid, jsontext.KindNull, jsontext.KindFalse, jsontext.KindTrue,
		jsontext.KindNumber, jsontext.KindEndObject, jsontext.KindBeginArray, jsontext.KindEndArray:
	}

	return fmt.Errorf(
		"%w: a path must be a string or a base64 object, not %v",
		errUnexpectedJSON,
		dec.PeekKind(),
	)
}

// MarshalJSONTo writes the integer as decimal text.
func (d DecimalU64) MarshalJSONTo(enc *jsontext.Encoder) error {
	//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	return enc.WriteToken(jsontext.String(strconv.FormatUint(uint64(d), decimalBase)))
}

// UnmarshalJSONFrom reads the integer from decimal text.
func (d *DecimalU64) UnmarshalJSONFrom(dec *jsontext.Decoder) error {
	text, err := readDecimalText(dec)
	if err != nil {
		return err
	}

	value, err := strconv.ParseUint(text, decimalBase, uint64Bits)
	if err != nil {
		return fmt.Errorf("termtrace: invalid unsigned 64-bit decimal %q: %w", text, err)
	}

	*d = DecimalU64(value)

	return nil
}

// Big returns the value as a [big.Int].
func (u Uint128) Big() *big.Int {
	value := new(big.Int).SetUint64(u.Hi)
	value.Lsh(value, uint64Bits)

	return value.Or(value, new(big.Int).SetUint64(u.Lo))
}

// String returns the value as decimal text.
func (u Uint128) String() string {
	return u.Big().String()
}

// MarshalJSONTo writes the integer as a JSON number.
func (u Uint128) MarshalJSONTo(enc *jsontext.Encoder) error {
	//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	return enc.WriteValue(jsontext.Value(u.String()))
}

// UnmarshalJSONFrom reads the integer from a JSON number.
func (u *Uint128) UnmarshalJSONFrom(dec *jsontext.Decoder) error {
	if dec.PeekKind() != '0' {
		return fmt.Errorf(
			"%w: an unsigned 128-bit integer must be a JSON number, not %v",
			errUnexpectedJSON,
			dec.PeekKind(),
		)
	}

	value, err := dec.ReadValue()
	if err != nil {
		return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	}

	parsed, err := parseUint128(string(value))
	if err != nil {
		return err
	}

	*u = parsed

	return nil
}

// MarshalJSONTo writes the integer as decimal text.
func (d DecimalU128) MarshalJSONTo(enc *jsontext.Encoder) error {
	//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	return enc.WriteToken(jsontext.String(d.String()))
}

// UnmarshalJSONFrom reads the integer from decimal text.
func (d *DecimalU128) UnmarshalJSONFrom(dec *jsontext.Decoder) error {
	text, err := readDecimalText(dec)
	if err != nil {
		return err
	}

	parsed, err := parseUint128(text)
	if err != nil {
		return err
	}

	d.Uint128 = parsed

	return nil
}

// Value returns the value, which is nil for an explicit null, and whether the field is present.
func (n Nullable[T]) Value() (*T, bool) {
	return n.value, n.present
}

// IsZero reports whether the field is left out.
func (n Nullable[T]) IsZero() bool {
	return !n.present
}

// MarshalJSONTo writes the value, or null.
func (n Nullable[T]) MarshalJSONTo(enc *jsontext.Encoder) error {
	if n.value == nil {
		//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
		return enc.WriteToken(jsontext.Null)
	}

	//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	return json.MarshalEncode(enc, n.value)
}

// UnmarshalJSONFrom reads the value, or null.
func (n *Nullable[T]) UnmarshalJSONFrom(dec *jsontext.Decoder) error {
	n.present = true
	if dec.PeekKind() == 'n' {
		n.value = nil
		_, err := dec.ReadToken()

		return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	}

	n.value = new(T)

	//nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	return json.UnmarshalDecode(dec, n.value)
}

// MarshalJSONTo writes the pair as a two-element array.
func (p Pair[A, B]) MarshalJSONTo(enc *jsontext.Encoder) error {
	if err := enc.WriteToken(jsontext.BeginArray); err != nil {
		return err //nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	}

	if err := json.MarshalEncode(enc, p.First); err != nil {
		return err //nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	}

	if err := json.MarshalEncode(enc, p.Second); err != nil {
		return err //nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	}

	//nolint:wrapcheck // json/v2 needs encoder errors unwrapped.
	return enc.WriteToken(jsontext.EndArray)
}

// UnmarshalJSONFrom reads a two-element array.
func (p *Pair[A, B]) UnmarshalJSONFrom(dec *jsontext.Decoder) error {
	if token, err := dec.ReadToken(); err != nil {
		return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	} else if token.Kind() != '[' {
		return fmt.Errorf("%w: a pair must be an array, not %v", errUnexpectedJSON, token.Kind())
	}

	if err := json.UnmarshalDecode(dec, &p.First); err != nil {
		return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	}

	if err := json.UnmarshalDecode(dec, &p.Second); err != nil {
		return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	}

	if token, err := dec.ReadToken(); err != nil {
		return err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	} else if token.Kind() != ']' {
		return fmt.Errorf("%w: a pair must have exactly two elements", errUnexpectedJSON)
	}

	return nil
}

func parseUint128(text string) (Uint128, error) {
	value, ok := new(big.Int).SetString(text, decimalBase)
	if !ok || value.Sign() < 0 || value.Cmp(maxUint128) > 0 {
		return Uint128{}, fmt.Errorf(
			"%w: invalid unsigned 128-bit integer %q",
			errUnexpectedJSON,
			text,
		)
	}

	mask := new(big.Int).SetUint64(^uint64(0))
	lo := new(big.Int).And(value, mask).Uint64()
	hi := new(big.Int).Rsh(value, uint64Bits).Uint64()

	return Uint128{Hi: hi, Lo: lo}, nil
}

func readDecimalText(dec *jsontext.Decoder) (string, error) {
	if dec.PeekKind() != '"' {
		return "", fmt.Errorf(
			"%w: a decimal integer must be a string, not %v",
			errUnexpectedJSON,
			dec.PeekKind(),
		)
	}

	token, err := dec.ReadToken()
	if err != nil {
		return "", err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	}

	return token.String(), nil
}

// readUnion reads the next value; it returns nil for JSON null.
func readUnion(dec *jsontext.Decoder) (jsontext.Value, error) {
	value, err := dec.ReadValue()
	if err != nil {
		return nil, err //nolint:wrapcheck // json/v2 needs decoder errors unwrapped.
	}

	if value.Kind() == 'n' {
		return nil, nil
	}

	return value, nil
}

// unionTag returns the compact JSON text of the member that selects a union variant.
func unionTag(raw jsontext.Value, member string) (string, error) {
	var members map[string]jsontext.Value
	if err := json.Unmarshal(raw, &members); err != nil {
		return "", fmt.Errorf("%w: union value: %w", errUnexpectedJSON, err)
	}

	value, ok := members[member]
	if !ok {
		return "", fmt.Errorf("%w: union value has no %q member", errUnexpectedJSON, member)
	}

	compact := jsontext.Value(bytes.Clone(value))
	if err := compact.Compact(); err != nil {
		return "", fmt.Errorf("%w: union %q member: %w", errUnexpectedJSON, member, err)
	}

	return string(compact), nil
}

// unionKey returns the quoted name of the only member of an externally tagged union value.
func unionKey(raw jsontext.Value) (string, error) {
	var members map[string]jsontext.Value
	if err := json.Unmarshal(raw, &members); err != nil {
		return "", fmt.Errorf("%w: union value: %w", errUnexpectedJSON, err)
	}

	if len(members) != 1 {
		return "", fmt.Errorf(
			"%w: union value must have one member, not %d",
			errUnexpectedJSON,
			len(members),
		)
	}

	for name := range members {
		return strconv.Quote(name), nil
	}

	return "", nil
}

// unionUnit reports whether raw is one of the bare-string variants of a union.
func unionUnit(raw jsontext.Value, units ...string) (string, bool, error) {
	if raw.Kind() != '"' {
		return "", false, nil
	}

	var text string
	if err := json.Unmarshal(raw, &text); err != nil {
		return "", false, fmt.Errorf("%w: union variant: %w", errUnexpectedJSON, err)
	}

	if slices.Contains(units, text) {
		return text, true, nil
	}

	return "", false, fmt.Errorf("%w: unknown union variant %q", errUnexpectedJSON, text)
}

func unknownVariant(union string, selector string) error {
	return fmt.Errorf("%w: unknown %s variant %s", errUnexpectedJSON, union, selector)
}
