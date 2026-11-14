package termtrace

import (
	"encoding/json/v2"
	"reflect"
	"testing"
)

func encode(t *testing.T, v any) string {
	t.Helper()

	data, err := json.Marshal(v)
	if err != nil {
		t.Fatalf("marshal %#v: %v", v, err)
	}

	return string(data)
}

func roundTrip[T any](t *testing.T, value T, want string) {
	t.Helper()

	if got := encode(t, value); got != want {
		t.Fatalf("marshal %#v = %s, want %s", value, got, want)
	}

	var decoded T
	if err := DecodeJSON([]byte(want), &decoded); err != nil {
		t.Fatalf("decode %s: %v", want, err)
	}

	if !reflect.DeepEqual(decoded, value) {
		t.Fatalf("decode %s = %#v, want %#v", want, decoded, value)
	}
}

func rejects[T any](t *testing.T, inputs ...string) {
	t.Helper()

	for _, input := range inputs {
		var decoded T
		if err := DecodeJSON([]byte(input), &decoded); err == nil {
			t.Errorf("decode %s into %T succeeded with %#v", input, decoded, decoded)
		}
	}
}

func TestPathKeepsExactBytes(t *testing.T) {
	roundTrip(t, Path("/tmp/demo"), `"/tmp/demo"`)
	roundTrip(t, Path("/tmp/\xffdemo"), `{"base64":"L3RtcC//ZGVtbw=="}`)
	rejects[Path](t, `7`, `["/tmp"]`)
}

func TestWideIntegersKeepFullRange(t *testing.T) {
	roundTrip(t, DecimalU64(18446744073709551615), `"18446744073709551615"`)
	rejects[DecimalU64](t, `18446744073709551615`, `"18446744073709551616"`, `"-1"`, `"1e3"`)

	maxValue := Uint128{Hi: ^uint64(0), Lo: ^uint64(0)}
	roundTrip(t, maxValue, `340282366920938463463374607431768211455`)
	roundTrip(t, Uint128{Hi: 1, Lo: 2}, `18446744073709551618`)
	rejects[Uint128](t, `340282366920938463463374607431768211456`, `-1`, `"5"`)

	roundTrip(t, DecimalU128{Uint128: maxValue}, `"340282366920938463463374607431768211455"`)
	rejects[DecimalU128](t, `5`, `"340282366920938463463374607431768211456"`)
}

func TestPairIsATwoElementArray(t *testing.T) {
	roundTrip(t, Pair[int, string]{First: 1, Second: "a"}, `[1,"a"]`)
	rejects[Pair[int, string]](t, `[1]`, `[1,"a",2]`, `{"first":1}`)
}

func TestNullableSeparatesNullFromAbsent(t *testing.T) {
	type holder struct {
		Color Nullable[RGB] `json:"color,omitzero"`
	}

	red := RGB{R: 255}

	roundTrip(t, holder{}, `{}`)
	roundTrip(t, holder{Color: Null[RGB]()}, `{"color":null}`)
	roundTrip(t, holder{Color: NonNull(red)}, `{"color":{"b":0,"g":0,"r":255}}`)

	var decoded holder
	if err := DecodeJSON([]byte(`{"color":null}`), &decoded); err != nil {
		t.Fatal(err)
	}

	if value, present := decoded.Color.Value(); !present || value != nil {
		t.Fatalf("explicit null decoded as %v, %v", value, present)
	}
}

func TestStyleFilterSendsFalseAndUnresolvedConstraints(t *testing.T) {
	filter := StyleFilter{Bold: new(false), ResolvedForeground: Null[RGB](), Underline: new(0)}
	roundTrip(t, filter, `{"bold":false,"resolved_foreground":null,"underline":0}`)
	roundTrip(t, StyleFilter{}, `{}`)
}

func TestUnionsDecodeEveryVariantShape(t *testing.T) {
	type holder struct {
		Color     ColorSpec        `json:"color,omitzero"`
		Condition LocatorCondition `json:"condition,omitzero"`
	}

	roundTrip(t, holder{}, `{}`)
	roundTrip(t, holder{Color: ColorSpecNone{}}, `{"color":{"kind":"none"}}`)
	roundTrip(
		t,
		holder{Color: ColorSpecPalette{Value: 9}},
		`{"color":{"kind":"palette","value":9}}`,
	)
	roundTrip(
		t,
		holder{Color: ColorSpecRgb{Value: RGB{G: 128}}},
		`{"color":{"kind":"rgb","value":{"b":0,"g":128,"r":0}}}`,
	)
	roundTrip(t, holder{Condition: LocatorConditionAbsent}, `{"condition":"absent"}`)
	roundTrip(t, holder{Condition: LocatorConditionCount{Count: 2}}, `{"condition":{"count":2}}`)

	rejects[holder](t,
		`{"color":{"kind":"cmyk"}}`,
		`{"color":{"value":9}}`,
		`{"condition":"sometimes"}`,
		`{"condition":{"count":1,"at_least":1}}`,
		`{"condition":{"more_than":1}}`,
	)
}
