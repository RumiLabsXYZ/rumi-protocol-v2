import Principal "mo:core/Principal";
import Result "mo:core/Result";

module {

  public type SourceCanisters = {
    var analytics : Principal;
    var backend : Principal;
  };

  public type SourceCanistersInit = {
    analytics : Principal;
    backend : Principal;
  };

  public func init(args : SourceCanistersInit) : SourceCanisters {
    {
      var analytics = args.analytics;
      var backend = args.backend;
    };
  };

  public func update(s : SourceCanisters, name : Text, id : Principal) : Result.Result<(), Text> {
    switch (name) {
      case "analytics" { s.analytics := id; #ok };
      case "backend" { s.backend := id; #ok };
      case _ { #err("unknown source canister: " # name) };
    };
  };

};
