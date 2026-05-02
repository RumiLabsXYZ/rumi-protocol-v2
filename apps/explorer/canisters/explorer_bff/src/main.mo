import Runtime "mo:core/Runtime";

persistent actor ExplorerBff {

  public query func ping() : async Text {
    "explorer_bff is alive"
  };

};
