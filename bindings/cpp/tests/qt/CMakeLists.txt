# Builds the Qt integration test: freshen.hpp used from a real Qt 6 program.
# Driven by ffi/tests/cpp_wrapper.rs; it can also be built by hand:
#
#   cmake -S bindings/cpp/tests/qt -B build -G Ninja \
#     -DFRESHEN_C_INCLUDE=ffi/include -DFRESHEN_CPP_INCLUDE=bindings/cpp/include \
#     -DFRESHEN_LIBRARY_DIR=target/debug
cmake_minimum_required(VERSION 3.16)
project(freshen_qt_test LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 REQUIRED COMPONENTS Core Concurrent)

add_executable(qt_test qt_test.cpp)
target_include_directories(qt_test PRIVATE
  ${FRESHEN_C_INCLUDE}
  ${FRESHEN_CPP_INCLUDE}
  ${CMAKE_CURRENT_SOURCE_DIR}/..)
target_link_directories(qt_test PRIVATE ${FRESHEN_LIBRARY_DIR})
target_link_libraries(qt_test PRIVATE Qt6::Core Qt6::Concurrent freshen_ffi)
set_target_properties(qt_test PROPERTIES BUILD_RPATH "${FRESHEN_LIBRARY_DIR}")
# Qt's own headers are clean under these flags; freshen.hpp must be too.
target_compile_options(qt_test PRIVATE -Wall -Wextra -Werror)
