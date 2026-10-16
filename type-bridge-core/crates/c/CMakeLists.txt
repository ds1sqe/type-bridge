cmake_minimum_required(VERSION 3.20)
# PROJECT_VERSION is the frozen ABI 1.6 package-metadata line: pkg-config
# reports it so generated packages pinned to `type-bridge = 1.6.0` keep
# resolving. The native ABI the runtime implements comes from the header;
# CMake reports it and serves every earlier minor down to 1.6 (see
# cmake/TypeBridgeConfigVersion.cmake.in), and pkg-config provides it as
# `type-bridge-abi`. The product/runtime version is reported at run time by
# type_bridge_runtime_version().
project(TypeBridge VERSION 1.6.0 LANGUAGES NONE)
set(TYPE_BRIDGE_C_ABI_OLDEST "1.6.0")
file(STRINGS "include/typebridge/type_bridge.h" TYPE_BRIDGE_C_ABI_MAJOR
     REGEX "^#define TYPE_BRIDGE_C_ABI_MAJOR [0-9]+u$")
file(STRINGS "include/typebridge/type_bridge.h" TYPE_BRIDGE_C_ABI_MINOR
     REGEX "^#define TYPE_BRIDGE_C_ABI_MINOR [0-9]+u$")
string(REGEX REPLACE "^.* ([0-9]+)u$" "\\1" TYPE_BRIDGE_C_ABI_MAJOR "${TYPE_BRIDGE_C_ABI_MAJOR}")
string(REGEX REPLACE "^.* ([0-9]+)u$" "\\1" TYPE_BRIDGE_C_ABI_MINOR "${TYPE_BRIDGE_C_ABI_MINOR}")
if(NOT TYPE_BRIDGE_C_ABI_MAJOR MATCHES "^[0-9]+$" OR
   NOT TYPE_BRIDGE_C_ABI_MINOR MATCHES "^[0-9]+$")
  message(FATAL_ERROR "the runtime header does not declare its native ABI version")
endif()
set(TYPE_BRIDGE_C_ABI_VERSION "${TYPE_BRIDGE_C_ABI_MAJOR}.${TYPE_BRIDGE_C_ABI_MINOR}.0")

include(CMakePackageConfigHelpers)
include(GNUInstallDirs)

string(REPLACE "\\" "/" TYPE_BRIDGE_C_INSTALL_LIBDIR "${CMAKE_INSTALL_LIBDIR}")
string(REPLACE "\\" "/" TYPE_BRIDGE_C_INSTALL_INCLUDEDIR "${CMAKE_INSTALL_INCLUDEDIR}")
string(REPLACE "\\" "/" TYPE_BRIDGE_C_INSTALL_BINDIR "${CMAKE_INSTALL_BINDIR}")
if(TYPE_BRIDGE_C_INSTALL_LIBDIR STREQUAL "" OR
   IS_ABSOLUTE "${TYPE_BRIDGE_C_INSTALL_LIBDIR}" OR
   TYPE_BRIDGE_C_INSTALL_LIBDIR MATCHES "(^|/)\\.\\.(/|$)")
  message(FATAL_ERROR
    "CMAKE_INSTALL_LIBDIR must be a non-empty relative path confined to the install prefix")
endif()
if(TYPE_BRIDGE_C_INSTALL_INCLUDEDIR STREQUAL "" OR
   IS_ABSOLUTE "${TYPE_BRIDGE_C_INSTALL_INCLUDEDIR}" OR
   TYPE_BRIDGE_C_INSTALL_INCLUDEDIR MATCHES "(^|/)\\.\\.(/|$)")
  message(FATAL_ERROR
    "CMAKE_INSTALL_INCLUDEDIR must be a non-empty relative path confined to the install prefix")
endif()
if(TYPE_BRIDGE_C_INSTALL_BINDIR STREQUAL "" OR
   IS_ABSOLUTE "${TYPE_BRIDGE_C_INSTALL_BINDIR}" OR
   TYPE_BRIDGE_C_INSTALL_BINDIR MATCHES "(^|/)\\.\\.(/|$)")
  message(FATAL_ERROR
    "CMAKE_INSTALL_BINDIR must be a non-empty relative path confined to the install prefix")
endif()
set(CMAKE_INSTALL_LIBDIR "${TYPE_BRIDGE_C_INSTALL_LIBDIR}")
set(CMAKE_INSTALL_INCLUDEDIR "${TYPE_BRIDGE_C_INSTALL_INCLUDEDIR}")
set(CMAKE_INSTALL_BINDIR "${TYPE_BRIDGE_C_INSTALL_BINDIR}")
file(
  RELATIVE_PATH
  TYPE_BRIDGE_PC_PREFIX_FROM_PCDIR
  "${CMAKE_INSTALL_PREFIX}/${TYPE_BRIDGE_C_INSTALL_LIBDIR}/pkgconfig"
  "${CMAKE_INSTALL_PREFIX}"
)

set(
  TYPE_BRIDGE_C_LIBRARY
  ""
  CACHE FILEPATH
  "Path to the Cargo-built TypeBridge C shared library"
)
if(NOT IS_ABSOLUTE "${TYPE_BRIDGE_C_LIBRARY}" OR
   NOT EXISTS "${TYPE_BRIDGE_C_LIBRARY}")
  message(FATAL_ERROR
    "TYPE_BRIDGE_C_LIBRARY must name an existing absolute shared-library path")
endif()

get_filename_component(
  TYPE_BRIDGE_C_LIBRARY_FILENAME "${TYPE_BRIDGE_C_LIBRARY}" NAME
)

if(WIN32)
  set(
    TYPE_BRIDGE_C_IMPORT_LIBRARY
    ""
    CACHE FILEPATH
    "Path to the Cargo-built TypeBridge C import library"
  )
  if(NOT IS_ABSOLUTE "${TYPE_BRIDGE_C_IMPORT_LIBRARY}" OR
     NOT EXISTS "${TYPE_BRIDGE_C_IMPORT_LIBRARY}")
    message(FATAL_ERROR
      "TYPE_BRIDGE_C_IMPORT_LIBRARY must name an existing absolute import-library path")
  endif()
  set(TYPE_BRIDGE_C_INSTALLED_IMPORT_LIBRARY "type_bridge_c.lib")
  install(
    FILES "${TYPE_BRIDGE_C_LIBRARY}"
    DESTINATION "${CMAKE_INSTALL_BINDIR}"
  )
  install(
    FILES "${TYPE_BRIDGE_C_IMPORT_LIBRARY}"
    DESTINATION "${CMAKE_INSTALL_LIBDIR}"
    RENAME "${TYPE_BRIDGE_C_INSTALLED_IMPORT_LIBRARY}"
  )
else()
  install(
    FILES "${TYPE_BRIDGE_C_LIBRARY}"
    DESTINATION "${CMAKE_INSTALL_LIBDIR}"
  )
endif()

install(
  FILES
    "include/typebridge/type_bridge.h"
  DESTINATION "${CMAKE_INSTALL_INCLUDEDIR}/typebridge"
)

configure_package_config_file(
  "cmake/TypeBridgeConfig.cmake.in"
  "${CMAKE_CURRENT_BINARY_DIR}/TypeBridgeConfig.cmake"
  INSTALL_DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/TypeBridge"
)
configure_file(
  "cmake/TypeBridgeConfigVersion.cmake.in"
  "${CMAKE_CURRENT_BINARY_DIR}/TypeBridgeConfigVersion.cmake"
  @ONLY
)
configure_file(
  "cmake/type-bridge.pc.in"
  "${CMAKE_CURRENT_BINARY_DIR}/type-bridge.pc"
  @ONLY
)

install(
  FILES
    "${CMAKE_CURRENT_BINARY_DIR}/TypeBridgeConfig.cmake"
    "${CMAKE_CURRENT_BINARY_DIR}/TypeBridgeConfigVersion.cmake"
  DESTINATION "${CMAKE_INSTALL_LIBDIR}/cmake/TypeBridge"
)
install(
  FILES "${CMAKE_CURRENT_BINARY_DIR}/type-bridge.pc"
  DESTINATION "${CMAKE_INSTALL_LIBDIR}/pkgconfig"
)
