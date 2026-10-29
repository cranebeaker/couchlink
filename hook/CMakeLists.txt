# Couchlink input hook: couchlink_hook.dll (injected into each game copy) and a small test target.
# Build one architecture per configure: use build.ps1, which runs this twice (x64 and x86).
cmake_minimum_required(VERSION 3.20)
project(couchlink_hook LANGUAGES C CXX)

if(NOT MSVC)
  message(FATAL_ERROR "The hook is built with MSVC (Visual Studio Build Tools).")
endif()

set(CL_ARCH "x64" CACHE STRING "Output folder name under hook/build (x64 or x86)")
set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
# The DLL runs inside arbitrary games: link the C runtime statically so no redistributable is needed.
set(CMAKE_MSVC_RUNTIME_LIBRARY "MultiThreaded")
set(CL_OUT_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/build" CACHE PATH "Folder that gets x64/ and x86/")
set(OUT_DIR "${CL_OUT_ROOT}/${CL_ARCH}")

set(MINHOOK_DIR "${CMAKE_CURRENT_SOURCE_DIR}/third_party/minhook")
set(MINHOOK_SRC
  ${MINHOOK_DIR}/src/buffer.c
  ${MINHOOK_DIR}/src/hook.c
  ${MINHOOK_DIR}/src/trampoline.c)
if(CL_ARCH STREQUAL "x64")
  list(APPEND MINHOOK_SRC ${MINHOOK_DIR}/src/hde/hde64.c)
else()
  list(APPEND MINHOOK_SRC ${MINHOOK_DIR}/src/hde/hde32.c)
endif()

# Keep build machine paths (user names) out of the binaries: no full PDB path, no source paths in __FILE__.
add_compile_options($<$<COMPILE_LANGUAGE:C,CXX>:/d1trimfile:${CMAKE_CURRENT_SOURCE_DIR}/> $<$<COMPILE_LANGUAGE:C,CXX>:/Brepro>)
add_link_options(/PDBALTPATH:%_PDB% /Brepro)

# Everything except DllMain is compiled once and shared by the injected DLL and the stand-in DLLs.
add_library(hook_core OBJECT
  src/common.cpp
  src/json.cpp
  src/xinput_hook.cpp
  src/focus_hook.cpp
  src/names_hook.cpp
  src/files_hook.cpp
  src/msg_hook.cpp
  src/kbm_hook.cpp
  src/kbm_dinput.cpp
  src/kbm_cursor.cpp
  src/child_hook.cpp
  ${MINHOOK_SRC})
target_include_directories(hook_core PRIVATE src ${MINHOOK_DIR}/include)
target_compile_definitions(hook_core PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN _CRT_SECURE_NO_WARNINGS)
target_compile_options(hook_core PRIVATE /W3 /utf-8 /EHsc /GS-)

# Version information (company, product, description) for every binary: unlabeled files look suspicious to scanners.
enable_language(RC)
enable_language(ASM_MASM)
function(cl_version_rc target description internal original out_var)
  set(CL_FILE_DESCRIPTION "${description}")
  set(CL_INTERNAL_NAME "${internal}")
  set(CL_ORIGINAL_NAME "${original}")
  set(rc "${CMAKE_CURRENT_BINARY_DIR}/rc/${target}.rc")
  configure_file(${CMAKE_CURRENT_SOURCE_DIR}/src/version.rc.in "${rc}" @ONLY)
  set(${out_var} "${rc}" PARENT_SCOPE)
endfunction()

function(cl_hook_dll target)
  add_library(${target} SHARED src/dllmain.cpp $<TARGET_OBJECTS:hook_core> ${ARGN})
  target_include_directories(${target} PRIVATE src ${MINHOOK_DIR}/include)
  target_compile_definitions(${target} PRIVATE UNICODE _UNICODE WIN32_LEAN_AND_MEAN _CRT_SECURE_NO_WARNINGS)
  target_compile_options(${target} PRIVATE $<$<COMPILE_LANGUAGE:C,CXX>:/W3 /utf-8 /EHsc /GS->)
  target_link_libraries(${target} PRIVATE user32 shell32 ole32 gdi32)
endfunction()

# 1. The injected DLL (fallback for games that load none of the stand-in names).
cl_version_rc(couchlink_hook "Couchlink input hook" "couchlink_hook" "couchlink_hook.dll" RC_HOOK)
cl_hook_dll(couchlink_hook ${RC_HOOK})

# 2. Stand-in DLLs: the game loads one of these by itself, so nothing has to reach into the game from outside.
set(CL_PROXY_NAMES version winmm dinput8 xinput1_3 xinput1_4 xinput9_1_0)
set(CL_PROXY_TARGETS "")
foreach(n ${CL_PROXY_NAMES})
  set(def "${CMAKE_CURRENT_SOURCE_DIR}/proxy/${n}-${CL_ARCH}.def")
  if(EXISTS "${def}")
    cl_version_rc(proxy_${n} "Couchlink input hook (loads as ${n}.dll)" "${n}" "${n}.dll" RC_P)
    cl_hook_dll(proxy_${n} "${def}" "${CMAKE_CURRENT_SOURCE_DIR}/proxy/${n}-${CL_ARCH}.asm"
                "${CMAKE_CURRENT_SOURCE_DIR}/proxy/${n}-${CL_ARCH}.cpp" ${RC_P})
    # The assembler writes its object into a folder named after the source's folder, and does not create it.
    foreach(cfg Release Debug RelWithDebInfo MinSizeRel)
      file(MAKE_DIRECTORY "${CMAKE_CURRENT_BINARY_DIR}/proxy_${n}.dir/${cfg}/proxy")
    endforeach()
    target_compile_definitions(proxy_${n} PRIVATE CL_PROXY)
    set_target_properties(proxy_${n} PROPERTIES OUTPUT_NAME ${n})
    list(APPEND CL_PROXY_TARGETS proxy_${n})
  endif()
endforeach()

add_executable(couchlink_test_target tests/test_target.cpp)
target_compile_definitions(couchlink_test_target PRIVATE UNICODE _UNICODE _CRT_SECURE_NO_WARNINGS)
target_compile_options(couchlink_test_target PRIVATE /W3 /utf-8 /EHsc)
target_link_libraries(couchlink_test_target PRIVATE user32 shell32 ole32 dinput8 dxguid)

foreach(t couchlink_hook ${CL_PROXY_TARGETS})
  if(t STREQUAL "couchlink_hook")
    set(dest "${OUT_DIR}")
  else()
    set(dest "${OUT_DIR}/proxy")
  endif()
  set_target_properties(${t} PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${dest}"
    RUNTIME_OUTPUT_DIRECTORY_DEBUG "${dest}"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${dest}"
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${dest}"
    PDB_OUTPUT_DIRECTORY "${dest}")
endforeach()
foreach(t couchlink_test_target)
  set_target_properties(${t} PROPERTIES
    RUNTIME_OUTPUT_DIRECTORY "${OUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY_DEBUG "${OUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY_RELEASE "${OUT_DIR}"
    RUNTIME_OUTPUT_DIRECTORY_RELWITHDEBINFO "${OUT_DIR}"
    PDB_OUTPUT_DIRECTORY "${OUT_DIR}")
endforeach()
set_target_properties(couchlink_test_target PROPERTIES OUTPUT_NAME test_target)
