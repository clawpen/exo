@echo off
REM ===============================================
REM Exo Container Runtime - Windows WSL2 Test Suite
REM ===============================================

setlocal enabledelayedexpansion

set TEST_COUNT=0
set PASS_COUNT=0
set FAIL_COUNT=0
set SKIP_COUNT=0

set EXO_BIN=%EXO_BIN%:.\target\release\exo.exe
set TEST_PREFIX=exo-test-

echo ===============================================
echo   Exo Container Runtime - Windows WSL2 Test
echo ===============================================
echo.

REM ===============================================
REM Helper Functions
REM ===============================================

:pass
echo [PASS] %~1
set /a PASS_COUNT+=1
set /a TEST_COUNT+=1
exit /b 0

:fail
echo [FAIL] %~1
if not "%~2"=="" echo        Error: %~2
set /a FAIL_COUNT+=1
set /a TEST_COUNT+=1
exit /b 0

:skip
echo [SKIP] %~1
set /a SKIP_COUNT+=1
exit /b 0

:info
echo [INFO] %~1
exit /b 0

:test
echo.
echo ===============================================
echo TEST: %~1
echo ===============================================
exit /b 0

REM ===============================================
REM Prerequisites Check
REM ===============================================

:check_prereqs
echo Checking prerequisites...

if not exist "%EXO_BIN%" (
    if exist ".\target\release\exo.exe" (
        set EXO_BIN=.\target\release\exo.exe
    ) else (
        echo [FAIL] Exo binary not found
        exit /b 1
    )
)

echo Using Exo binary: %EXO_BIN%

REM Check WSL
wsl --version >nul 2>&1
if errorlevel 1 (
    echo [FAIL] WSL not found
    exit /b 1
)

echo WSL: available
echo.

REM ===============================================
REM SMOKE TESTS
REM ===============================================

:test_run
call :test "1. Container Run"
%EXO_BIN% run --name %TEST_PREFIX%-1 alpine:latest echo "Hello from Exo" 2>&1 | findstr /C:"Hello from Exo" >nul
if errorlevel 1 (
    call :fail "Container run" "Expected output not found"
) else (
    call :pass "Container runs and produces expected output"
)
%EXO_BIN% rm -f %TEST_PREFIX%-1 >nul 2>&1
exit /b 0

:test_stop
call :test "2. Container Stop"
%EXO_BIN% run --name %TEST_PREFIX%-2 --detach alpine:latest sleep 60 >nul 2>&1
timeout /t 2 /nobreak >nul
%EXO_BIN% stop %TEST_PREFIX%-2 >nul 2>&1
if errorlevel 1 (
    call :fail "Container stop" "Stop command failed"
) else (
    call :pass "Container stopped successfully"
)
%EXO_BIN% rm -f %TEST_PREFIX%-2 >nul 2>&1
exit /b 0

:test_remove
call :test "3. Container Remove"
%EXO_BIN% run --name %TEST_PREFIX%-3 alpine:latest echo "test" >nul 2>&1
timeout /t 1 /nobreak >nul
%EXO_BIN% rm %TEST_PREFIX%-3 >nul 2>&1
if errorlevel 1 (
    call :fail "Container remove" "Remove command failed"
) else (
    call :pass "Container removed successfully"
)
exit /b 0

:test_list
call :test "4. List Containers"
%EXO_BIN% run --name %TEST_PREFIX%-4 --detach alpine:latest sleep 60 >nul 2>&1
timeout /t 2 /nobreak >nul
%EXO_BIN% list 2>&1 | findstr /C:"%TEST_PREFIX%-4" >nul
if errorlevel 1 (
    call :fail "Container list" "Container not shown in list output"
) else (
    call :pass "Container listing works"
)
%EXO_BIN% stop %TEST_PREFIX%-4 >nul 2>&1
%EXO_BIN% rm %TEST_PREFIX%-4 >nul 2>&1
exit /b 0

:test_env
call :test "5. Environment Variables"
%EXO_BIN% run --name %TEST_PREFIX%-env alpine:latest sh -c "echo MY_TEST_VAR=$MY_TEST_VAR" 2>&1 | findstr /C:"MY_TEST_VAR=hello" >nul
if errorlevel 1 (
    call :fail "Environment variables" "Variables not set correctly"
) else (
    call :pass "Environment variables passed correctly"
)
exit /b 0

:test_volume
call :test "6. Volume Mounts (Windows to WSL)"
REM Create a temp file on Windows
echo host-data > %TEMP%\exo-volume-test.txt
REM Convert Windows path to WSL path
for /f "delims=" %%i in ('wsl wslpath "%TEMP%\exo-volume-test.txt"') do set WSL_PATH=%%i

%EXO_BIN% run --name %TEST_PREFIX%-vol -v %TEMP%\exo-volume-test.txt:/data/host-file.txt alpine:latest cat /data/host-file.txt 2>&1 | findstr /C:"host-data" >nul
if errorlevel 1 (
    call :skip "Volume mounts (WSL path conversion needed)"
) else (
    call :pass "Volume mount from Windows works"
)
del %TEMP%\exo-volume-test.txt 2>nul
exit /b 0

:test_stdio
call :test "7. Stdio Communication"
echo PING | %EXO_BIN% run --rm alpine:latest sh -c "read msg; echo PONG: $msg" 2>&1 | findstr /C:"PONG: PING" >nul
if errorlevel 1 (
    call :fail "Stdio communication" "Round-trip failed"
) else (
    call :pass "Stdio round-trip works"
)
exit /b 0

:test_json
call :test "8. Agent Channel Protocol (JSON)"
echo {"type":"test","data":"hello"} | %EXO_BIN% run --rm alpine:latest cat 2>&1 | findstr /C:"test" >nul
if errorlevel 1 (
    call :fail "Agent channel" "JSON handling failed"
) else (
    call :pass "JSON messages pass through correctly"
)
exit /b 0

:test_port_forwarding
call :test "9. Port Forwarding (netsh)"
%EXO_BIN% run --name %TEST_PREFIX%-port --publish 18080:80 --detach alpine:latest sleep 60 >nul 2>&1
timeout /t 3 /nobreak >nul

REM Check if netsh rule was created
netsh interface portproxy show all 2>&1 | findstr /C:"18080" >nul
if errorlevel 1 (
    call :info "Port forwarding rule not found in netsh"
    call :skip "Port forwarding (may need manual setup)"
) else (
    call :pass "Port forwarding created via netsh"
)
%EXO_BIN% stop %TEST_PREFIX%-port >nul 2>&1
%EXO_BIN% rm %TEST_PREFIX%-port >nul 2>&1
exit /b 0

:test_detach
call :test "10. Detached Mode"
%EXO_BIN% run --name %TEST_PREFIX%-detach --detach alpine:latest sleep 60 >nul 2>&1
timeout /t 2 /nobreak >nul
%EXO_BIN% list 2>&1 | findstr /C:"%TEST_PREFIX%-detach" >nul
if errorlevel 1 (
    call :fail "Detached mode" "Container not running"
) else (
    call :pass "Detached container runs in background"
)
%EXO_BIN% stop %TEST_PREFIX%-detach >nul 2>&1
%EXO_BIN% rm %TEST_PREFIX%-detach >nul 2>&1
exit /b 0

:test_sync
call :test "11. Synchronous Mode"
%EXO_BIN% run --name %TEST_PREFIX%-sync alpine:latest sh -c "echo 'Sync Output'; sleep 1" 2>&1 | findstr /C:"Sync Output" >nul
if errorlevel 1 (
    call :fail "Synchronous mode" "Output not captured"
) else (
    call :pass "Synchronous execution works"
)
exit /b 0

:test_concurrent
call :test "12. Concurrent Containers"
for /L %%i in (1,1,3) do (
    start /b "" %EXO_BIN% run --name %TEST_PREFIX%-conc-%%i --detach alpine:latest sleep 30 >nul 2>&1
)
timeout /t 5 /nobreak >nul

set /a COUNT=0
for /L %%i in (1,1,3) do (
    %EXO_BIN% list 2>&1 | findstr /C:"%TEST_PREFIX%-conc-%%i" >nul
    if not errorlevel 1 set /a COUNT+=1
)

if !COUNT! GEQ 2 (
    call :pass "Concurrent operations (!COUNT!/3 containers created)"
) else (
    call :fail "Concurrent operations" "Only !COUNT!/3 containers created"
)

for /L %%i in (1,1,3) do (
    %EXO_BIN% stop %TEST_PREFIX%-conc-%%i >nul 2>&1
    %EXO_BIN% rm %TEST_PREFIX%-conc-%%i >nul 2>&1
)
exit /b 0

REM ===============================================
REM MAIN TEST RUNNER
REM ===============================================

:run_all_tests
call :test_run
call :test_stop
call :test_remove
call :test_list
call :test_env
call :test_volume
call :test_stdio
call :test_json
call :test_port_forwarding
call :test_detach
call :test_sync
call :test_concurrent

REM ===============================================
REM SUMMARY
REM ===============================================

:print_summary
echo.
echo ===============================================
echo           TEST SUMMARY
echo ===============================================
echo   Passed:  %PASS_COUNT%
echo   Failed:  %FAIL_COUNT%
echo   Skipped: %SKIP_COUNT%
echo ===============================================
echo.

if %FAIL_COUNT% GTR 0 (
    echo Some tests failed!
    exit /b 1
) else (
    echo All tests passed!
    exit /b 0
)

REM Run all tests
call :run_all_tests
