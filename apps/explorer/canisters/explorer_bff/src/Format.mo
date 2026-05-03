import Float "mo:core/Float";
import Int64 "mo:core/Int64";
import Nat8 "mo:core/Nat8";
import T "Types";

module {

  public func e8s(amount : Nat64, decimals : Nat8, symbol : Text) : T.FormattedNumber {
    let divisor : Float = Float.pow(10.0, Float.fromInt(Nat8.toNat(decimals)));
    let asFloat : Float = Float.fromInt64(Int64.fromNat64(amount));
    let scaled : Float = asFloat / divisor;
    let formatted : Text = formatFloat(scaled, 2) # (if (symbol == "") "" else " " # symbol);
    {
      raw_e8s = amount;
      decimals = decimals;
      formatted = formatted;
    };
  };

  // Minimal float formatter: rounds to `places` decimals and converts to text via Float.format.
  // Uses #fix format for predictable decimal places.
  // This is an interim formatter; Plan 5 (lenses) will replace with proper number formatting.
  func formatFloat(value : Float, places : Nat) : Text {
    let prec : Nat8 = Nat8.fromNat(places);
    Float.format(value, #fix prec);
  };

};
