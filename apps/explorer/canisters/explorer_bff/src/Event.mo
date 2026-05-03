import Time "mo:core/Time";
import Text "mo:core/Text";
import Iter "mo:core/Iter";
import Char "mo:core/Char";
import Nat32 "mo:core/Nat32";
import Nat64 "mo:core/Nat64";
import T "Types";
import ST "SourceTypes";
import SA "SourceActors";
import SourceConfig "SourceConfig";

module {

  func textToNat64(t : Text) : ?Nat64 {
    var n : Nat64 = 0;
    var any = false;
    for (c in t.chars()) {
      let cp : Nat32 = Char.toNat32(c);
      if (cp >= 48 and cp <= 57) {
        n := n * 10 + Nat64.fromNat(Nat32.toNat(cp - 48));
        any := true;
      } else {
        return null;
      };
    };
    if (any) ?n else null;
  };

  func parseGlobalId(global_id : Text) : ?(Text, Nat64) {
    let parts = Iter.toArray(Text.split(global_id, #char ':'));
    if (parts.size() != 2) return null;
    switch (textToNat64(parts[1])) {
      case null null;
      case (?idx) ?(parts[0], idx);
    };
  };

  func defaultEvent(global_id : Text) : T.EventDetailDTO {
    {
      global_id = global_id;
      source = "backend";
      source_event_id = 0;
      kind = "unknown";
      timestamp_ns = 0;
      payload_summary = "Event not found";
      payload_json = "{}";
      related_event_ids = [];
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

  public func fetch(sources : SourceConfig.SourceCanisters, global_id : Text) : async T.EventDetailDTO {
    switch (parseGlobalId(global_id)) {
      case null defaultEvent(global_id);
      case (?(_source, idx)) {
        let backendActor = SA.backend(sources.backend);
        let resp = await backendActor.get_events_filtered({
          start = idx;
          length = 1 : Nat64;
          types = null;
          principal = null;
          collateral_token = null;
          time_range = null;
          min_size_e8s = null;
          admin_labels = null;
        });
        if (resp.events.size() == 0) return defaultEvent(global_id);
        let ev = resp.events[0];
        {
          global_id = global_id;
          source = "backend";
          source_event_id = ev.global_index;
          kind = ev.kind;
          timestamp_ns = ev.timestamp_ns;
          payload_summary = ev.payload_summary;
          payload_json = "{\"global_index\":" # debug_show(ev.global_index) # ",\"kind\":\"" # ev.kind # "\"}";
          related_event_ids = [];
          generated_at_ns = Nat64.fromIntWrap(Time.now());
        };
      };
    };
  };

};
