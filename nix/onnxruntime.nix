# Pre-built ONNX Runtime for use with the `ort` crate.
{
  lib,
  stdenv,
  fetchurl,
  autoPatchelfHook,
}: let
  version = "1.20.0";

  platform =
    {
      x86_64-linux = {
        name = "linux-x64";
        hash = "0c2rgf5sc6zzz8z90ris9ap4g6nw3nslacpnhcpbhr724a5x8w5a";
      };
      aarch64-darwin = {
        name = "osx-arm64";
        hash = "1bnx406hnidw1njq73mhg4nvprgxbw9jzbz3g17sk8zhkzxamkrb";
      };
    }
    .${
      stdenv.hostPlatform.system
    }
      or (throw "onnxruntime-bin: unsupported system ${stdenv.hostPlatform.system}");
in
  stdenv.mkDerivation {
    pname = "onnxruntime-bin";
    inherit version;

    src = fetchurl {
      url = "https://github.com/microsoft/onnxruntime/releases/download/v${version}/onnxruntime-${platform.name}-${version}.tgz";
      hash = "sha256:${platform.hash}";
    };

    sourceRoot = "onnxruntime-${platform.name}-${version}";

    nativeBuildInputs = lib.optionals stdenv.hostPlatform.isLinux [autoPatchelfHook];
    buildInputs = lib.optionals stdenv.hostPlatform.isLinux [stdenv.cc.cc.lib];

    installPhase = ''
      runHook preInstall
      mkdir -p $out/lib $out/include
      cp -r lib/*.${
        if stdenv.hostPlatform.isDarwin
        then "dylib*"
        else "so*"
      } $out/lib/
      cp -r include/* $out/include/
      runHook postInstall
    '';

    meta = {
      description = "ONNX Runtime ${version} pre-built binaries";
      homepage = "https://github.com/microsoft/onnxruntime";
      license = lib.licenses.mit;
      platforms = ["x86_64-linux" "aarch64-darwin"];
    };
  }
