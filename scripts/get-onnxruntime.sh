#!/usr/bin/env bash
# Downloads Microsoft's build of ONNX Runtime (the version the app is built
# against) into a directory: the CPU build as the fallback runtime, or with
# --cuda the GPU build for NVIDIA cards. That build needs NVIDIA's CUDA 12
# and cuDNN 9 libraries; the ones the system does not already have are
# downloaded next to it (NVIDIA's own builds, from PyPI, checksummed), so
# only the driver has to be installed. AMD uses the distribution's
# onnxruntime-rocm.
#
#   scripts/get-onnxruntime.sh [dir]           CPU, default dir ~/.local/lib/voice-changer
#   scripts/get-onnxruntime.sh --cuda [dir]    CUDA, into <dir>/cuda
#   scripts/get-onnxruntime.sh --arch aarch64 [dir]   another architecture (packaging)
set -euo pipefail
VERSION="${ORT_VERSION:-1.22.0}"
CUDA=0
MACHINE="$(uname -m)"
while [[ "${1:-}" == --* ]]; do
    case "$1" in
        --cuda) CUDA=1; shift ;;
        --arch) MACHINE="$2"; shift 2 ;;
        *) echo "unknown option $1" >&2; exit 2 ;;
    esac
done
# The app looks in ~/.local/lib/voice-changer (and its cuda/ subfolder).
DIR="${1:-$HOME/.local/lib/voice-changer}"
if [[ $CUDA == 1 ]]; then DIR="$DIR/cuda"; fi
case "$MACHINE" in
    x86_64) ARCH=x64 ;;
    aarch64) ARCH=aarch64 ;;
    arm64) ARCH=aarch64 ;;
    *) echo "unsupported architecture $MACHINE" >&2; exit 1 ;;
esac
# Microsoft's NuGet packages: plain zip files with the libraries in
# runtimes/linux-*/native. (The GitHub release archives this used to fetch
# went away with the repository in November 2026; the libraries in here are
# byte for byte the same.) Checksums are for the version above; another
# ORT_VERSION is downloaded unchecked.
NUGET="https://api.nuget.org/v3-flatcontainer"
case "$ARCH" in x64) RID=linux-x64 ;; *) RID=linux-arm64 ;; esac
if [[ $CUDA == 1 ]]; then
    if [[ "$ARCH" != x64 ]]; then
        echo "Microsoft publishes the CUDA build of ONNX Runtime for x86_64 only." >&2
        exit 1
    fi
    PKG=microsoft.ml.onnxruntime.gpu.linux
    SUM=d3dabd8235b7a04a67838a3b93437420f1a709528a8d8611e08bedbb08206d1d
else
    PKG=microsoft.ml.onnxruntime
    SUM=d571e63a2329baacb713f441e65ad75284de354db6e1ac435fe4bebbb417986a
fi
if [[ "$VERSION" != 1.22.0 ]]; then SUM=""; fi
URL="$NUGET/$PKG/$VERSION/$PKG.$VERSION.nupkg"
mkdir -p "$DIR"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
unpack_runtime() { # package, directory: the libraries for this machine
    if command -v unzip >/dev/null; then
        unzip -q -j -o "$1" "runtimes/$RID/native/*.so" -d "$2"
    elif command -v python3 >/dev/null; then
        python3 -c 'import os, sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
for n in z.namelist():
    if n.startswith("runtimes/" + sys.argv[3] + "/native/") and n.endswith(".so"):
        with open(os.path.join(sys.argv[2], os.path.basename(n)), "wb") as f:
            f.write(z.read(n))' "$1" "$2" "$RID"
    else
        echo "unzip (or python3) is needed to unpack ONNX Runtime" >&2
        return 1
    fi
}
if [[ $CUDA == 1 && -f "$DIR/libonnxruntime.so.$VERSION" && -f "$DIR/libonnxruntime_providers_cuda.so" ]]; then
    # A second run only fetches what an interrupted first one left out.
    echo "ONNX Runtime $VERSION (CUDA): already in $DIR"
else
    echo "downloading $URL"
    curl -fL --retry 3 --progress-bar -o "$TMP/ort.zip" "$URL"
    if [[ -n "$SUM" ]] && ! echo "$SUM  $TMP/ort.zip" | sha256sum -c --quiet -; then
        echo "the download of ONNX Runtime is damaged (checksum mismatch); run this again" >&2
        exit 1
    fi
    mkdir "$TMP/libs"
    unpack_runtime "$TMP/ort.zip" "$TMP/libs"
    rm -f "$TMP/ort.zip"
    # The packages carry the library without its version; the app reads the
    # version from the file name.
    mv "$TMP/libs/libonnxruntime.so" "$TMP/libs/libonnxruntime.so.$VERSION"
    chmod 755 "$TMP/libs"/*.so*
    if [[ $CUDA == 1 ]]; then
        # The CUDA provider is a separate library the runtime loads from its
        # own folder; without it the GPU can never be used.
        cp "$TMP/libs"/libonnxruntime*.so* "$DIR/"
    else
        cp "$TMP/libs/libonnxruntime.so.$VERSION" "$DIR/"
    fi
fi
ln -sf "libonnxruntime.so.$VERSION" "$DIR/libonnxruntime.so.1" 2>/dev/null || true
ln -sf "libonnxruntime.so.$VERSION" "$DIR/libonnxruntime.so" 2>/dev/null || true
if [[ $CUDA != 1 ]]; then
    echo "ONNX Runtime $VERSION installed in $DIR"
    exit 0
fi
echo "ONNX Runtime $VERSION (CUDA) installed in $DIR"
echo "Next: NVIDIA's CUDA 12 and cuDNN 9 libraries, up to 1.7 GB to download and up to 3 GB on disk."

# NVIDIA's libraries the CUDA build links against. Distributions that have
# moved on to CUDA 13 no longer package these (Fedora 42 and later), so they
# come from NVIDIA's wheels on PyPI: plain zip files with the libraries in
# nvidia/*/lib. The versions are a set PyTorch ships together.
#   what | library that shows it is present | sha256 | URL
PYPI="https://files.pythonhosted.org/packages"
NVIDIA_LIBS=(
    "CUDA runtime 12.8|libcudart.so.12|adade8dcbd0edf427b7204d480d6066d33902cab2a4707dcfc48a2d0fd44ab90|$PYPI/0d/9b/a997b638fcd068ad6e4d53b8551a7d30fe8b404d6f1804abf1df69838932/nvidia_cuda_runtime_cu12-12.8.90-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl"
    "cuBLAS 12.8 (594 MB)|libcublas.so.12|8ac4e771d5a348c551b2a426eda6193c19aa630236b418086020df5ba9667142|$PYPI/dc/61/e24b560ab2e2eaeb3c839129175fb330dfcfc29e5203196e5541a4c44682/nvidia_cublas_cu12-12.8.4.1-py3-none-manylinux_2_27_x86_64.whl"
    "nvJitLink 12.8 (39 MB)|libnvJitLink.so.12|81ff63371a7ebd6e6451970684f916be2eab07321b73c9d244dc2b4da7f73b88|$PYPI/f6/74/86a07f1d0f42998ca31312f998bd3b9a7eff7f52378f4f270c8679c77fb9/nvidia_nvjitlink_cu12-12.8.93-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl"
    "cuFFT 11.3 (193 MB)|libcufft.so.11|4d2dd21ec0b88cf61b62e6b43564355e5222e4a3fb394cac0db101f2dd0d4f74|$PYPI/1f/13/ee4e00f30e676b66ae65b4f08cb5bcbb8392c03f54f2d5413ea99a5d1c80/nvidia_cufft_cu12-11.3.3.83-py3-none-manylinux2014_x86_64.manylinux_2_17_x86_64.whl"
    "cuRAND 10.3 (63 MB)|libcurand.so.10|b32331d4f4df5d6eefa0554c565b626c7216f87a06a4f56fab27c3b68a830ec9|$PYPI/fb/aa/6584b56dc84ebe9cf93226a5cde4d99080c8e90ab40f0c27bda7a0f29aa1/nvidia_curand_cu12-10.3.9.90-py3-none-manylinux_2_27_x86_64.whl"
    "NVRTC 12.8 (88 MB)|libnvrtc.so.12|a7756528852ef889772a84c6cd89d41dfa74667e24cca16bb31f8f061e3e9994|$PYPI/05/6b/32f747947df2da6994e999492ab306a903659555dddc0fbdeb9d71f75e52/nvidia_cuda_nvrtc_cu12-12.8.93-py3-none-manylinux2010_x86_64.manylinux_2_12_x86_64.whl"
    "cuDNN 9.10 (706 MB)|libcudnn.so.9|949452be657fa16687d0930933f032835951ef0892b37d2d53824d1a84dc97a8|$PYPI/ba/51/e123d997aa098c61d029f76663dedbfb9bc8dcf8c60cbd6adbe42f76d049/nvidia_cudnn_cu12-9.10.2.21-py3-none-manylinux_2_27_x86_64.whl"
)
if [[ "$ARCH" != x64 ]]; then
    echo "The CUDA 12 runtime and cuDNN 9 are not downloaded on $MACHINE; install them from your distribution."
    exit 0
fi

# What the system's loader already knows (ldconfig is in /sbin on Debian).
LDCACHE="$( { /sbin/ldconfig -p || ldconfig -p; } 2>/dev/null || true)"
on_system() { grep -q "^[[:space:]]*$1 " <<<"$LDCACHE"; }

# Wheels are large: keep them off /tmp (often memory) and next to where the
# libraries go, so the final move is a rename.
WORK="$DIR/.download"
rm -rf "$WORK"
trap 'rm -rf "$TMP" "$WORK"' EXIT
unpack() { # wheel, directory: the shared libraries only
    if command -v unzip >/dev/null; then
        unzip -q -j -o "$1" 'nvidia/*/lib/*.so*' -d "$2"
    elif command -v python3 >/dev/null; then
        python3 -c 'import os, sys, zipfile
z = zipfile.ZipFile(sys.argv[1])
for n in z.namelist():
    b = os.path.basename(n)
    if n.startswith("nvidia/") and "/lib/" in n and ".so" in b:
        with open(os.path.join(sys.argv[2], b), "wb") as f:
            f.write(z.read(n))' "$1" "$2"
    else
        echo "unzip (or python3) is needed to unpack NVIDIA's libraries" >&2
        return 1
    fi
}
for entry in "${NVIDIA_LIBS[@]}"; do
    IFS='|' read -r what probe sum url <<<"$entry"
    if [[ -f "$DIR/$probe" ]]; then
        echo "$what: already in $DIR"
    elif on_system "$probe"; then
        echo "$what: found on the system ($probe)"
    else
        echo "downloading NVIDIA $what"
        mkdir -p "$WORK/libs"
        curl -fL --retry 3 --progress-bar -o "$WORK/wheel.zip" "$url"
        if ! echo "$sum  $WORK/wheel.zip" | sha256sum -c --quiet -; then
            echo "the download of $what is damaged (checksum mismatch); run this again" >&2
            exit 1
        fi
        unpack "$WORK/wheel.zip" "$WORK/libs"
        rm -f "$WORK/wheel.zip"
        # Not used by ONNX Runtime: 220 MB less on disk.
        rm -f "$WORK/libs"/*.alt.so* "$WORK/libs"/libnvblas.so* "$WORK/libs"/libcufftw.so*
        mv "$WORK/libs"/* "$DIR/"
        rmdir "$WORK/libs"
    fi
done
echo "NVIDIA's libraries stay under NVIDIA's own licence (in each nvidia-*-cu12 package on pypi.org)."
echo "Only the NVIDIA driver has to come from your distribution."
echo "Run 'voice-changer doctor' to check, then restart the app. To remove everything: rm -r \"$DIR\""
