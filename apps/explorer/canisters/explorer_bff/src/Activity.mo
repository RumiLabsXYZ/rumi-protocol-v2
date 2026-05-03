import T "Types";
import SourceConfig "SourceConfig";

module {

  // Activity feed requires get_events_filtered on the backend, which returns a
  // full Event variant (not EventSummary) on mainnet — causing a decode trap.
  // Returns an empty feed. main.mo wraps this in try/catch as well.
  //
  // This lights up once the full Event variant is ported into BFF shadow types.
  public func fetch(
    _sources : SourceConfig.SourceCanisters,
    filter : T.ActivityFilter,
    _cursor : T.ActivityCursor,
  ) : async T.ActivityFeedDTO {
    {
      events = [];
      next_cursor = null;
      total_estimated = 0;
      filters_applied = filter;
    };
  };

};
