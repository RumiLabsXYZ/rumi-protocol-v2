import Time "mo:core/Time";
import Nat64 "mo:core/Nat64";
import T "Types";
import SourceConfig "SourceConfig";

module {

  // Event detail requires get_events_filtered on the backend, which returns a
  // full Event variant not yet decoded by the BFF shadow types. Returns a
  // graceful "not yet available" DTO.
  // main.mo wraps this in try/catch as an extra safety net.
  public func fetch(_sources : SourceConfig.SourceCanisters, global_id : Text) : async T.EventDetailDTO {
    {
      global_id = global_id;
      source = "backend";
      source_event_id = 0;
      kind = "unknown";
      timestamp_ns = 0;
      payload_summary = "Event detail not yet available (Event variant porting in progress)";
      payload_json = "{}";
      related_event_ids = [];
      generated_at_ns = Nat64.fromIntWrap(Time.now());
    };
  };

};
