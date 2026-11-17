cmake_minimum_required(VERSION 3.20)
project(amacs LANGUAGES C CXX)

set(SOURCES
  main.c
  util.c
)

# Warnings in debug builds.
if(CMAKE_BUILD_TYPE STREQUAL "Debug")
  add_compile_options(-Wall -Wextra)
else()
  message(STATUS "release")
endif()

foreach(src ${SOURCES})
  message(STATUS "${src}")
endforeach()

add_executable(amacs ${SOURCES})
