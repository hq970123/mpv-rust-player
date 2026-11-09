# Injected into whisper.cpp's CMake run (CMAKE_PROJECT_INCLUDE_BEFORE, set by CI)
# so ggml is built for a portable CPU baseline instead of the build runner's
# own CPU (-mcpu=native / -march=native). Binaries then run on any user's
# machine, and the i8mm detection bug on Apple arm64 runners is avoided.
set(GGML_NATIVE OFF CACHE BOOL "portable build" FORCE)
