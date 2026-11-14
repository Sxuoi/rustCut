# Third-party notices

The Windows build of RustCut includes `ffmpeg.exe` and `ffprobe.exe` from FFmpeg. These are not part of
RustCut. The Windows build means `RustCut-windows-x64-setup.exe` and the folder it installs. RustCut runs
both programs as separate processes.

## FFmpeg 9.0.2

| | |
| :--- | :--- |
| Files | `ffmpeg.exe`, `ffprobe.exe` |
| Build | `ffmpeg version 9.0.2-full_build-www.gyan.dev`: the "release full" static build of FFmpeg 9.0.2 by Gyan Doshi (<https://www.gyan.dev/ffmpeg/builds/>), published at <https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2> |
| License | GNU General Public License, version 3 or (at your option) any later version. The full text is in [licenses/FFmpeg-GPL-3.0.txt](licenses/FFmpeg-GPL-3.0.txt), which is FFmpeg's own `COPYING.GPLv3`. The installer puts this file next to the programs. |
| Copyright | Copyright (c) 2000-2026 the FFmpeg developers, and the authors of the libraries the build includes |

The build is configured with `--enable-gpl --enable-version3` and includes GPL libraries such as libx264
and libx265, so the two programs as a whole are licensed under GPL version 3 or later. FFmpeg is
distributed WITHOUT ANY WARRANTY; see sections 15 and 16 of the license.

### Source code

- **FFmpeg 9.0.2**, which this build is made from:
  - the tag `n9.0.2`, commit `946fcce07b6dcd0331c8cc609192aeff5e1924f8`, at
    <https://github.com/FFmpeg/FFmpeg/tree/n9.0.2>;
  - or the release archive <https://ffmpeg.org/releases/ffmpeg-9.0.2.tar.xz>.
- **The build itself:** <https://www.gyan.dev/ffmpeg/builds/>. Its release section links its source to the
  commit above.
- **The external libraries:** the configuration below lists which ones the build includes. Each library is
  published by its own project.

### Build configuration

As reported by `ffmpeg -version`:

```text
--enable-gpl --enable-version3 --enable-static --disable-w32threads --disable-autodetect
--enable-cairo --enable-fontconfig --enable-iconv --enable-gnutls --enable-lcms2 --enable-libxml2
--enable-gmp --enable-bzlib --enable-lzma --enable-libsnappy --enable-zlib --enable-librist
--enable-libsrt --enable-libssh --enable-libzmq --enable-avisynth --enable-libbluray
--enable-libcaca --enable-libdvdnav --enable-libdvdread --enable-sdl2 --enable-libaribb24
--enable-libaribcaption --enable-libdav1d --enable-libdavs2 --enable-libopenjpeg --enable-libquirc
--enable-libuavs3d --enable-libxevd --enable-libzvbi --enable-liboapv --enable-libqrencode
--enable-librav1e --enable-libsvtav1 --enable-libvvenc --enable-libwebp --enable-libx264
--enable-libx265 --enable-libxavs2 --enable-libxeve --enable-libxvid --enable-libaom --enable-libjxl
--enable-libsvtjpegxs --enable-libvpx --enable-mediafoundation --enable-libass --enable-frei0r
--enable-libfreetype --enable-libfribidi --enable-libharfbuzz --enable-liblensfun
--enable-libvidstab --enable-libvmaf --enable-libzimg --enable-amf --enable-cuda-llvm --enable-cuvid
--enable-dxva2 --enable-d3d11va --enable-d3d12va --enable-ffnvcodec --enable-libvpl --enable-nvdec
--enable-nvenc --enable-vaapi --enable-vulkan --enable-libplacebo --enable-opencl --enable-libcdio
--enable-openal --enable-libgme --enable-libmodplug --enable-libopenmpt --enable-libopencore-amrwb
--enable-libmp3lame --enable-libshine --enable-libtheora --enable-libtwolame --enable-libvo-amrwbenc
--enable-libcodec2 --enable-libilbc --enable-libgsm --enable-liblc3 --enable-libopencore-amrnb
--enable-libopus --enable-libspeex --enable-libvorbis --enable-ladspa --enable-libbs2b
--enable-libflite --enable-libmysofa --enable-librubberband --enable-libsoxr --enable-chromaprint
--enable-whisper
```
