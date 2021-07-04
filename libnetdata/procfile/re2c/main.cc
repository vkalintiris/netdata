// re2c $INPUT -o $OUTPUT 
#include <cassert>
#include <vector>
#include <cstring>

#include <fstream>
#include <sstream>
#include <iostream>

#include "proc_pid_io.h"
#include "proc_pid_stat.h"
#include "proc_pid_status.h"
#include "proc_stat.h"

int main(int argc, char *argv[])
{
    (void) argc;
    (void) argv;

    std::ifstream IFS("/proc/self/status");
    std::stringstream Buffer;
    Buffer << IFS.rdbuf();

    std::string Contents = Buffer.str();
    std::cout << "Contents:\n" << Contents << std::endl;

    char *BufCp = strdup(Contents.c_str());

    proc_pid_status_t pid_status;
    proc_pid_status(BufCp, &pid_status);

    std::cout << "euid: " << pid_status.euid << std::endl;
    std::cout << "egid: " << pid_status.egid << std::endl;
    std::cout << "vm_size: " << pid_status.vm_size << std::endl;
    std::cout << "vm_rss: " << pid_status.vm_rss << std::endl;
    std::cout << "rss_file: " << pid_status.rss_file << std::endl;
    std::cout << "rss_shmem: " << pid_status.rss_shmem << std::endl;
    std::cout << "vm_swap: " << pid_status.vm_swap << std::endl;
    return 0;
}
