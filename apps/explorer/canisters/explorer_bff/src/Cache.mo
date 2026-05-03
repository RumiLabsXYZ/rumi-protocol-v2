import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";

module {

  public class TtlCache<T>(ttl_ns : Int) {
    var value : ?T = null;
    var written_at : Int = 0;

    public func get() : ?T {
      switch value {
        case null null;
        case (?v) {
          let now = Time.now();
          if (now - written_at > ttl_ns) null else ?v;
        };
      };
    };

    public func getStale() : ?T {
      // Returns whatever's cached even if past TTL.
      // Useful for serving the user something while the next refresh is pending.
      value;
    };

    public func set(v : T) {
      value := ?v;
      written_at := Time.now();
    };

    public func ageMs() : Nat64 {
      let now = Time.now();
      Nat64.fromIntWrap((now - written_at) / 1_000_000);
    };

  };

};
