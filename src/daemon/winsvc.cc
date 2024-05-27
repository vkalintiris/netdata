/*
 * TODO:
 *       - Check slow start/stop times
 *           Notes: (a) It seems that we can just transition from start_pending to running
 *                  (b) It seems we can block ServiceMain() indefinitely...
 *       - Exit and never resume
 *       - Call and adapt main()
*/
#include "libnetdata/libnetdata.h"

#include <windows.h>
#include <iostream>
#include <fstream>
#include <chrono>

extern "C" int netdata_main(int argc, char *argv[]);

static SERVICE_STATUS_HANDLE svc_status_handle = nullptr;
static HANDLE g_ServiceStopEvent = INVALID_HANDLE_VALUE;

static void WriteLog(const std::string& message)
{
    SYSTEMTIME time;
    GetSystemTime(&time);
    
    std::ofstream log("/opt/netdata/service.log", std::ios_base::app);
    log << time.wHour << ":" << time.wMinute << ":" << time.wSecond << " - " << message << std::endl;
}

static void WorkerThread()
{
    WriteLog("Worker thread started.");
    while (WaitForSingleObject(g_ServiceStopEvent, 5000) == WAIT_TIMEOUT)
    {
        WriteLog("Hello, World!");
    }
    
    WriteLog("Worker thread stopping.");
}

static void WINAPI ServiceControlHandler(DWORD controlCode)
{
    SERVICE_STATUS svc_status = {};
    
    switch (controlCode)
    {
        case SERVICE_CONTROL_STOP:
            WriteLog("ServiceControlHandler(SERVICE_CONTROL_STOP)");
            if (svc_status_handle) {
                svc_status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
                svc_status.dwCurrentState = SERVICE_STOP_PENDING;
                svc_status.dwControlsAccepted = 0;
                SetServiceStatus(svc_status_handle, &svc_status);
            
                SetEvent(g_ServiceStopEvent);
            
                svc_status.dwCurrentState = SERVICE_STOPPED;
                SetServiceStatus(svc_status_handle, &svc_status);
            }
            break;
        case SERVICE_START: {
            WriteLog("ServiceControlHandler(SERVICE_START)");
            svc_status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
            svc_status.dwCurrentState = SERVICE_START_PENDING;
            svc_status.dwControlsAccepted = SERVICE_ACCEPT_STOP;
            SetServiceStatus(svc_status_handle, &svc_status);
    
            break;        
        }
        default:
            break;
    }
}

void WINAPI ServiceMain(DWORD argc, LPSTR* argv)
{
    WriteLog("Called ServiceMain()");
    
    svc_status_handle = RegisterServiceCtrlHandler("Netdata", ServiceControlHandler);
    if (!svc_status_handle) {
        return;
    }

    SERVICE_STATUS svc_status = {};
    
    svc_status.dwServiceType = SERVICE_WIN32_OWN_PROCESS;
    svc_status.dwCurrentState = SERVICE_START_PENDING;
    svc_status.dwControlsAccepted = SERVICE_ACCEPT_STOP;
    svc_status.dwWin32ExitCode = 0;
    svc_status.dwServiceSpecificExitCode = 0;
    svc_status.dwCheckPoint = 0;
    svc_status.dwWaitHint = 0;
    
    if (!SetServiceStatus(svc_status_handle, &svc_status))
    {
        WriteLog("ServiceMain() failed to set service status to START_PENDING.");
        return;
    }
    
    g_ServiceStopEvent = CreateEvent(nullptr, TRUE, FALSE, nullptr);
    if (g_ServiceStopEvent == INVALID_HANDLE_VALUE)
    {
        WriteLog("ServiceMain() failed to create service stop event.");
        svc_status.dwCurrentState = SERVICE_STOPPED;
        SetServiceStatus(svc_status_handle, &svc_status);
        return;
    }
    
    svc_status.dwCurrentState = SERVICE_RUNNING;
    if (!SetServiceStatus(svc_status_handle, &svc_status)) {
        WriteLog("ServiceMain() failed to set service status to SERVICE_RUNNING.");
        return;
    }

    int nd_argc = 3;
    char *nd_argv[] = {strdupz("/usr/bin/netdata"), strdupz("-D"), NULL};
    netdata_main(nd_argc, nd_argv);

    WriteLog("[LosAlamos] WTF!!!");
    
#if 0
    WaitForSingleObject(g_ServiceStopEvent, INFINITE);
    CloseHandle(g_ServiceStopEvent);
    g_ServiceStopEvent = INVALID_HANDLE_VALUE;
    
    serviceStatus.dwCurrentState = SERVICE_STOPPED;
    SetServiceStatus(g_ServiceStatusHandle, &serviceStatus);
#endif
}

int main() {
    WriteLog("Called main()");
    
    SERVICE_TABLE_ENTRY serviceTable[] = {
        { "Netdata", ServiceMain },
        { nullptr, nullptr }
    };
    
    if (!StartServiceCtrlDispatcher(serviceTable)) {
        DWORD ret = GetLastError();
        
        char Buf[1024];
        snprintf(Buf, 1024, "Failed to start service control dispatcher: %u", ret);
        WriteLog(Buf);
        
        return ret;
    }
    
    return 0;
}
