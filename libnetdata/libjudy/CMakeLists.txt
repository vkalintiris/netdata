# SPDX-License-Identifier: GPL-3.0-or-later

add_executable(judyltablesgen src/JudyL/JudyLTablesGen.c)
target_include_directories(judyltablesgen PRIVATE src src/JudyCommon)
target_compile_options(judyltablesgen PRIVATE -Wno-format -Wno-format-security)
target_compile_definitions(judyltablesgen PRIVATE JU_64BIT JUDYL)

add_custom_command(
  OUTPUT ${CMAKE_CURRENT_BINARY_DIR}/JudyLTables.c
  COMMAND judyltablesgen DEPENDS judyltablesgen
)

set(LIBJUDY_SOURCES src/Judy.h
                    src/JudyCommon/JudyMalloc.c
                    src/JudyCommon/JudyPrivate.h
                    src/JudyCommon/JudyPrivate1L.h
                    src/JudyCommon/JudyPrivateBranch.h
                    src/JudyL/JudyL.h
                    src/JudyL/JudyLByCount.c
                    src/JudyL/JudyLCascade.c
                    src/JudyL/JudyLCount.c
                    src/JudyL/JudyLCreateBranch.c
                    src/JudyL/JudyLDecascade.c
                    src/JudyL/JudyLDel.c
                    src/JudyL/JudyLFirst.c
                    src/JudyL/JudyLFreeArray.c
                    src/JudyL/j__udyLGet.c
                    src/JudyL/JudyLGet.c
                    src/JudyL/JudyLInsArray.c
                    src/JudyL/JudyLIns.c
                    src/JudyL/JudyLInsertBranch.c
                    src/JudyL/JudyLMallocIF.c
                    src/JudyL/JudyLMemActive.c
                    src/JudyL/JudyLMemUsed.c
                    src/JudyL/JudyLNext.c
                    src/JudyL/JudyLNextEmpty.c
                    src/JudyL/JudyLPrev.c
                    src/JudyL/JudyLPrevEmpty.c
                    src/JudyHS/JudyHS.c
                    JudyLTables.c
)

add_library(judy STATIC ${LIBJUDY_SOURCES})

target_include_directories(judy PRIVATE src src/JudyCommon)
target_compile_definitions(judy PRIVATE JU_64BIT JUDYL)
target_compile_options(judy PRIVATE -Wno-sign-compare -Wno-implicit-fallthrough)

set(LIBJUDY_PREV_FILES src/JudyL/JudyLPrev.c
                       src/JudyL/JudyLPrevEmpty.c)

set(LIBJUDY_NEXT_FILES src/JudyL/JudyLNext.c
                       src/JudyL/JudyLNextEmpty.c)

set_source_files_properties(${LIBJUDY_PREV_FILES} PROPERTIES COMPILE_OPTIONS "-DJUDYPREV")
set_source_files_properties(${LIBJUDY_NEXT_FILES} PROPERTIES COMPILE_OPTIONS "-DJUDYNEXT")
set_source_files_properties(src/JudyL/j__udyLGet.c PROPERTIES COMPILE_OPTIONS "-DJUDYGETINLINE")
set_source_files_properties(src/JudyL/JudyLByCount.c PROPERTIES COMPILE_OPTIONS "-DNOSMARTJBB -DNOSMARTJBU -DNOSMARTJLB")
set_source_files_properties(JudyLTables.c PROPERTIES COMPILE_OPTIONS "-I${CMAKE_CURRENT_SOURCE_DIR}/src/JudyL")
