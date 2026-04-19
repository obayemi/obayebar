{naersk}: final: prev: {
  obayebar = final.callPackage ./default.nix {
    naersk = final.callPackage naersk {
      cargo = final.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);
      rustc = final.rust-bin.selectLatestNightlyWith (toolchain: toolchain.default);
    };
  };
}
