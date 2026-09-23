cmake_minimum_required(VERSION 3.16)

project(fcitx5-mimir VERSION 0.1.0)


set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Find dependencies
find_package(ECM REQUIRED)
set(CMAKE_MODULE_PATH ${ECM_MODULE_PATH} ${CMAKE_CURRENT_SOURCE_DIR}/cmake)

find_package(Fcitx5Core 5.0 REQUIRED)
find_package(Fcitx5Module COMPONENTS Notifications QuickPhrase)
find_package(PkgConfig REQUIRED)
pkg_check_modules(XKBCommon REQUIRED xkbcommon)

include(FeatureSummary)
include(GNUInstallDirs)
include(ECMSetupVersion)
include(ECMUninstallTarget)

# Use fcitx5 compiler settings
include("${FCITX_INSTALL_CMAKECONFIG_DIR}/Fcitx5Utils/Fcitx5CompilerSettings.cmake")

# Path to the Rust library
set(MIMIR_RUST_LIB "${CMAKE_CURRENT_SOURCE_DIR}/../../../target/release/libmimir_fcitx5.so")
set(MIMIR_INCLUDE_DIR "${CMAKE_CURRENT_SOURCE_DIR}/../include")

# Build Rust library as a custom target
find_program(CARGO cargo REQUIRED)

# The addon is normally built on the machine it runs on, so optimize for its
# CPU by default (measured ~5% faster conversion). Turn OFF when building a
# binary for distribution; an externally set RUSTFLAGS always wins.
option(MIMIR_NATIVE "Optimize for the build machine's CPU (-C target-cpu=native)" ON)
set(MIMIR_CARGO_ENV "")
if(MIMIR_NATIVE AND NOT DEFINED ENV{RUSTFLAGS})
    set(MIMIR_CARGO_ENV ${CMAKE_COMMAND} -E env "RUSTFLAGS=-C target-cpu=native")
endif()

add_custom_target(mimir_rust_lib ALL
    COMMAND ${MIMIR_CARGO_ENV} ${CARGO} build --release -p mimir-fcitx5
    WORKING_DIRECTORY "${CMAKE_CURRENT_SOURCE_DIR}/../../.."
    COMMENT "Building mimir-fcitx5 Rust library"
    BYPRODUCTS "${MIMIR_RUST_LIB}"
)

# Add the addon
add_library(mimir MODULE
    src/mimir.cpp
)
add_dependencies(mimir mimir_rust_lib)

target_include_directories(mimir PRIVATE
    ${MIMIR_INCLUDE_DIR}
    ${CMAKE_CURRENT_SOURCE_DIR}/src
    ${XKBCommon_INCLUDE_DIRS}
)

# Expose fcitx5 version to C++ so we can guard newer API usage
if(Fcitx5Core_VERSION VERSION_GREATER_EQUAL "5.1.9")
    target_compile_definitions(mimir PRIVATE FCITX5_HAS_CANDIDATE_SET_COMMENT)
endif()

target_link_libraries(mimir
    Fcitx5::Core
    Fcitx5::Config
    ${MIMIR_RUST_LIB}
    ${XKBCommon_LIBRARIES}
)

# Remove lib prefix (fcitx5 expects "mimir.so" not "libmimir.so")
set_target_properties(mimir PROPERTIES PREFIX "")

# Set RPATH so fcitx5 can find libmimir_fcitx5.so at runtime
# $ORIGIN means "same directory as this library"
set_target_properties(mimir PROPERTIES
    BUILD_RPATH "${CMAKE_CURRENT_SOURCE_DIR}/../../../target/release"
    INSTALL_RPATH "\$ORIGIN"
    BUILD_WITH_INSTALL_RPATH FALSE
)

# Install the addon
install(TARGETS mimir DESTINATION "${FCITX_INSTALL_ADDONDIR}")

# Install Rust library alongside the addon
install(FILES "${MIMIR_RUST_LIB}" DESTINATION "${FCITX_INSTALL_ADDONDIR}")

# Install addon config
install(FILES mimir-addon.conf.in DESTINATION "${FCITX_INSTALL_PKGDATADIR}/addon"
    RENAME mimir.conf)

# Install input method config
install(FILES mimir.conf DESTINATION "${FCITX_INSTALL_PKGDATADIR}/inputmethod")

# Install metainfo
install(FILES org.fcitx.Fcitx5.Addon.Mimir.metainfo.xml.in
    DESTINATION "${CMAKE_INSTALL_DATAROOTDIR}/metainfo"
    RENAME org.fcitx.Fcitx5.Addon.Mimir.metainfo.xml)

# Install icons at multiple sizes for XDG icon theme
install(FILES icons/fcitx-mimir-16.png
    DESTINATION "${CMAKE_INSTALL_DATAROOTDIR}/icons/hicolor/16x16/apps/"
    RENAME fcitx-mimir.png)
install(FILES icons/fcitx-mimir-24.png
    DESTINATION "${CMAKE_INSTALL_DATAROOTDIR}/icons/hicolor/24x24/apps/"
    RENAME fcitx-mimir.png)
install(FILES icons/fcitx-mimir-32.png
    DESTINATION "${CMAKE_INSTALL_DATAROOTDIR}/icons/hicolor/32x32/apps/"
    RENAME fcitx-mimir.png)
install(FILES icons/fcitx-mimir-48.png
    DESTINATION "${CMAKE_INSTALL_DATAROOTDIR}/icons/hicolor/48x48/apps/"
    RENAME fcitx-mimir.png)
install(FILES icons/fcitx-mimir-128.png
    DESTINATION "${CMAKE_INSTALL_DATAROOTDIR}/icons/hicolor/128x128/apps/"
    RENAME fcitx-mimir.png)

feature_summary(WHAT ALL FATAL_ON_MISSING_REQUIRED_PACKAGES)
