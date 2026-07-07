#import "ExoVMM.h"
#import <Virtualization/Virtualization.h>
#import <fcntl.h>
#import <sys/socket.h>
#import <sys/time.h>
#import <unistd.h>

@interface ExoVM : NSObject
@property (strong) VZVirtualMachine *vm;
@property (strong) dispatch_queue_t vmQueue;
@property (copy) NSString *lastError;
@property (strong) NSLock *errorLock;
@property (assign) BOOL startCompleted;
@property (assign) int rpcHostReadFd;
@property (assign) int rpcHostWriteFd;
@end

@implementation ExoVM

- (instancetype)init {
    self = [super init];
    if (self) {
        self.errorLock = [[NSLock alloc] init];
        self.vmQueue = dispatch_queue_create("ca.clawpen.exo.vm", DISPATCH_QUEUE_SERIAL);
        self.rpcHostReadFd = -1;
        self.rpcHostWriteFd = -1;
    }
    return self;
}

- (void)setLastErrorFromError:(NSError *)error message:(NSString *)message {
    [self.errorLock lock];
    if (error) {
        self.lastError = [NSString stringWithFormat:@"%@: %@", message, error.localizedDescription];
    } else {
        self.lastError = message;
    }
    [self.errorLock unlock];
}

- (const char *)lastErrorUTF8 {
    [self.errorLock lock];
    const char *s = self.lastError ? [self.lastError UTF8String] : "";
    [self.errorLock unlock];
    return s;
}

- (void)pumpRunLoopUntilDate:(NSDate *)date {
    while ([date timeIntervalSinceNow] > 0) {
        [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode
                                 beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.05]];
    }
}

- (BOOL)createWithKernel:(NSString *)kernel
                  initrd:(NSString *)initrd
                    disk:(NSString *)disk
          consoleLogPath:(NSString *)consoleLogPath
                  memory:(uint64_t)memory
                    cpus:(uint32_t)cpus
                    name:(NSString *)name {
    @autoreleasepool {
        NSURL *kernelURL = [NSURL fileURLWithPath:kernel];
        NSURL *initrdURL = [NSURL fileURLWithPath:initrd];
        NSURL *diskURL = [NSURL fileURLWithPath:disk];

        NSLog(@"ExoVMM: creating VM kernel=%@ initrd=%@ disk=%@ console=%@ memory=%llu cpus=%u", kernel, initrd, disk, consoleLogPath, memory, cpus);
        VZLinuxBootLoader *bootLoader = [[VZLinuxBootLoader alloc] initWithKernelURL:kernelURL];
        bootLoader.initialRamdiskURL = initrdURL;
        bootLoader.commandLine = @"rw console=hvc0 init=/init";

        VZVirtualMachineConfiguration *config = [[VZVirtualMachineConfiguration alloc] init];
        config.bootLoader = bootLoader;
        config.memorySize = memory;
        config.CPUCount = cpus;

        // Note: vftool (a working reference) does not set a platform config.
        // Keep the default platform for the boot prototype.

        NSError *error = nil;

        // Optional block device backed by a raw disk image.
        if (disk.length > 0) {
            NSURL *diskURL = [NSURL fileURLWithPath:disk];
            VZDiskImageStorageDeviceAttachment *diskAttachment =
                [[VZDiskImageStorageDeviceAttachment alloc] initWithURL:diskURL readOnly:NO error:&error];
            if (!diskAttachment) {
                [self setLastErrorFromError:error message:@"Failed to attach disk image"];
                return NO;
            }
            VZVirtioBlockDeviceConfiguration *blockDevice =
                [[VZVirtioBlockDeviceConfiguration alloc] initWithAttachment:diskAttachment];
            config.storageDevices = @[blockDevice];
        }

        // Serial port logging to file for debugging boot issues.
        NSMutableArray *serialPorts = [NSMutableArray array];
        if (consoleLogPath.length > 0) {
            NSURL *logURL = [NSURL fileURLWithPath:consoleLogPath];
            NSLog(@"ExoVMM: creating serial log attachment at %@", logURL);
            VZFileSerialPortAttachment *serialAttachment =
                [[VZFileSerialPortAttachment alloc] initWithURL:logURL append:YES error:&error];
            if (serialAttachment) {
                VZVirtioConsoleDeviceSerialPortConfiguration *serialPort =
                    [[VZVirtioConsoleDeviceSerialPortConfiguration alloc] init];
                serialPort.attachment = serialAttachment;
                [serialPorts addObject:serialPort];
                NSLog(@"ExoVMM: serial log port configured");
            } else {
                [self setLastErrorFromError:error message:@"Failed to create serial log attachment"];
                return NO;
            }
        }

        // Dedicated bidirectional serial port for host-guest JSON RPC.
        int sv[2];
        if (socketpair(AF_UNIX, SOCK_STREAM, 0, sv) != 0) {
            [self setLastErrorFromError:nil message:@"Failed to create RPC socketpair"];
            return NO;
        }
        // sv[0] is given to the VM; sv[1] stays on the host.
        NSFileHandle *vmRead = [[NSFileHandle alloc] initWithFileDescriptor:sv[0]];
        NSFileHandle *vmWrite = [[NSFileHandle alloc] initWithFileDescriptor:sv[0]];
        VZFileHandleSerialPortAttachment *rpcAttachment =
            [[VZFileHandleSerialPortAttachment alloc]
                initWithFileHandleForReading:vmRead
                fileHandleForWriting:vmWrite];
        VZVirtioConsoleDeviceSerialPortConfiguration *rpcPort =
            [[VZVirtioConsoleDeviceSerialPortConfiguration alloc] init];
        rpcPort.attachment = rpcAttachment;
        [serialPorts addObject:rpcPort];
        self.rpcHostReadFd = sv[1];
        self.rpcHostWriteFd = sv[1];
        NSLog(@"ExoVMM: RPC serial port configured (host fd=%d)", sv[1]);

        if (serialPorts.count > 0) {
            config.serialPorts = serialPorts;
        }

        VZNetworkDeviceAttachment *nda = [[VZNATNetworkDeviceAttachment alloc] init];
        VZVirtioNetworkDeviceConfiguration *net_conf = [[VZVirtioNetworkDeviceConfiguration alloc] init];
        net_conf.attachment = nda;
        config.networkDevices = @[net_conf];

        VZVirtioSocketDeviceConfiguration *socketDevice =
            [[VZVirtioSocketDeviceConfiguration alloc] init];
        config.socketDevices = @[socketDevice];

        VZVirtioEntropyDeviceConfiguration *entropy =
            [[VZVirtioEntropyDeviceConfiguration alloc] init];
        config.entropyDevices = @[entropy];

        if (@available(macOS 12.0, *)) {
            VZGenericPlatformConfiguration *platform = [[VZGenericPlatformConfiguration alloc] init];
            config.platform = platform;
        }

        if (![config validateWithError:&error]) {
            [self setLastErrorFromError:error message:@"Invalid VM configuration"];
            return NO;
        }

        self.vm = [[VZVirtualMachine alloc] initWithConfiguration:config queue:self.vmQueue];
        // Delegate intentionally omitted for the initial boot prototype.
        return YES;
    }
}

- (BOOL)start {
    @autoreleasepool {
        if (self.vm.state == VZVirtualMachineStateRunning) {
            return YES;
        }
        __block BOOL canStart = NO;
        dispatch_sync(self.vmQueue, ^{
            canStart = self.vm.canStart;
            NSLog(@"ExoVMM: starting VM (canStart=%d state=%ld)", canStart, (long)self.vm.state);
        });
        // Wait briefly for the VM to finish asynchronous setup.
        for (int i = 0; i < 50 && !canStart; i++) {
            [NSThread sleepForTimeInterval:0.05];
            dispatch_sync(self.vmQueue, ^{
                canStart = self.vm.canStart;
            });
        }
        NSLog(@"ExoVMM: canStart after wait=%d", canStart);
        if (!canStart) {
            [self setLastErrorFromError:nil message:@"VM is not startable"];
            return NO;
        }
        __block BOOL completed = NO;
        __block NSError *startError = nil;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        dispatch_async(self.vmQueue, ^{
            [self.vm startWithCompletionHandler:^(NSError *error) {
                NSLog(@"ExoVMM: start completion handler called error=%@", error);
                startError = error;
                completed = YES;
                dispatch_semaphore_signal(sem);
            }];
        });
        dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, 120 * NSEC_PER_SEC);
        if (dispatch_semaphore_wait(sem, deadline) != 0) {
            [self setLastErrorFromError:nil message:@"Timeout starting VM"];
            return NO;
        }
        if (startError) {
            [self setLastErrorFromError:startError message:@"Failed to start VM"];
            return NO;
        }
        return YES;
    }
}

- (BOOL)stop {
    @autoreleasepool {
        if (self.vm.state != VZVirtualMachineStateRunning) {
            return YES;
        }
        __block BOOL completed = NO;
        __block NSError *stopError = nil;
        dispatch_semaphore_t sem = dispatch_semaphore_create(0);
        dispatch_async(self.vmQueue, ^{
            [self.vm stopWithCompletionHandler:^(NSError *error) {
                NSLog(@"ExoVMM: stop completion handler called error=%@", error);
                stopError = error;
                completed = YES;
                dispatch_semaphore_signal(sem);
            }];
        });
        dispatch_time_t deadline = dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC);
        if (dispatch_semaphore_wait(sem, deadline) != 0) {
            [self setLastErrorFromError:nil message:@"Timeout stopping VM"];
            return NO;
        }
        if (stopError) {
            [self setLastErrorFromError:stopError message:@"Failed to stop VM"];
            return NO;
        }
        return completed;
    }
}

- (NSString *)requestOnPort:(uint32_t)port
                       json:(NSString *)json
                    timeout:(uint32_t)timeoutMs
                      error:(NSError **)error {
    VZVirtioSocketDevice *socketDevice = (VZVirtioSocketDevice *)self.vm.socketDevices.firstObject;
    if (!socketDevice) {
        if (error) {
            *error = [NSError errorWithDomain:@"ExoVMM" code:1
                                     userInfo:@{NSLocalizedDescriptionKey: @"No socket device"}];
        }
        return nil;
    }

    __block VZVirtioSocketConnection *connection = nil;
    __block NSError *connectError = nil;
    __block BOOL connected = NO;
    dispatch_semaphore_t connectSem = dispatch_semaphore_create(0);

    [socketDevice connectToPort:port completionHandler:^(VZVirtioSocketConnection *conn, NSError *err) {
        connection = conn;
        connectError = err;
        connected = YES;
        dispatch_semaphore_signal(connectSem);
    }];

    dispatch_time_t connectDeadline = dispatch_time(DISPATCH_TIME_NOW, (int64_t)(timeoutMs * NSEC_PER_MSEC));
    dispatch_semaphore_wait(connectSem, connectDeadline);

    if (connectError) {
        if (error) *error = connectError;
        return nil;
    }
    if (!connection) {
        if (error) {
            *error = [NSError errorWithDomain:@"ExoVMM" code:2
                                     userInfo:@{NSLocalizedDescriptionKey: @"Timeout connecting to guest agent"}];
        }
        return nil;
    }

    int fd = connection.fileDescriptor;
    struct timeval tv;
    tv.tv_sec = timeoutMs / 1000;
    tv.tv_usec = (timeoutMs % 1000) * 1000;
    setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
    setsockopt(fd, SOL_SOCKET, SO_SNDTIMEO, &tv, sizeof(tv));

    int flags = fcntl(fd, F_GETFL, 0);
    if (flags != -1) {
        fcntl(fd, F_SETFL, flags & ~O_NONBLOCK);
    }

    NSString *payload = [NSString stringWithFormat:@"%@\n", json];
    NSData *payloadData = [payload dataUsingEncoding:NSUTF8StringEncoding];
    const uint8_t *bytes = payloadData.bytes;
    size_t total = payloadData.length;
    size_t written = 0;
    while (written < total) {
        ssize_t n = write(fd, bytes + written, total - written);
        if (n < 0) {
            if (errno == EINTR) continue;
            if (error) {
                *error = [NSError errorWithDomain:NSPOSIXErrorDomain code:errno
                                         userInfo:@{NSLocalizedDescriptionKey: @"Write to guest failed"}];
            }
            return nil;
        }
        written += n;
    }

    NSMutableData *responseData = [NSMutableData data];
    char byte;
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeoutMs / 1000.0];
    while ([deadline timeIntervalSinceNow] > 0) {
        ssize_t n = read(fd, &byte, 1);
        if (n < 0) {
            if (errno == EINTR) continue;
            if (error) {
                *error = [NSError errorWithDomain:NSPOSIXErrorDomain code:errno
                                         userInfo:@{NSLocalizedDescriptionKey: @"Read from guest failed"}];
            }
            return nil;
        }
        if (n == 0) {
            if (error) {
                *error = [NSError errorWithDomain:@"ExoVMM" code:6
                                         userInfo:@{NSLocalizedDescriptionKey: @"Guest closed connection"}];
            }
            return nil;
        }
        if (byte == '\n') break;
        [responseData appendBytes:&byte length:1];
    }

    if ([deadline timeIntervalSinceNow] <= 0) {
        if (error) {
            *error = [NSError errorWithDomain:@"ExoVMM" code:5
                                     userInfo:@{NSLocalizedDescriptionKey: @"Timeout reading guest response"}];
        }
        return nil;
    }

    NSString *str = [[NSString alloc] initWithData:responseData encoding:NSUTF8StringEncoding];
    return [str stringByTrimmingCharactersInSet:[NSCharacterSet whitespaceAndNewlineCharacterSet]];
}

@end

#pragma mark - C ABI

exo_vm_t exo_vm_create(const char *kernel_path,
                       const char *initrd_path,
                       const char *disk_path,
                       const char *console_log_path,
                       uint64_t memory_bytes,
                       uint32_t cpu_count,
                       const char *vm_name) {
    @autoreleasepool {
        ExoVM *exoVM = [[ExoVM alloc] init];
        BOOL ok = [exoVM createWithKernel:[NSString stringWithUTF8String:kernel_path]
                                   initrd:[NSString stringWithUTF8String:initrd_path]
                                     disk:[NSString stringWithUTF8String:disk_path]
                           consoleLogPath:[NSString stringWithUTF8String:console_log_path ?: ""]
                                   memory:memory_bytes
                                     cpus:cpu_count
                                     name:[NSString stringWithUTF8String:vm_name ?: "exo-vm"]];
        // Always return the allocated handle so Rust can read last_error even on
        // configuration failures. Rust is responsible for freeing the handle.
        return (__bridge_retained void *)exoVM;
    }
}

void exo_vm_free(exo_vm_t vm) {
    @autoreleasepool {
        if (!vm) return;
        ExoVM *exoVM = (__bridge_transfer ExoVM *)vm;
        if (exoVM.vm.state == VZVirtualMachineStateRunning) {
            [exoVM stop];
        }
        exoVM.vm = nil;
    }
}

int exo_vm_start(exo_vm_t vm) {
    @autoreleasepool {
        if (!vm) return -1;
        ExoVM *exoVM = (__bridge ExoVM *)vm;
        return [exoVM start] ? 0 : -1;
    }
}

int exo_vm_stop(exo_vm_t vm) {
    @autoreleasepool {
        if (!vm) return -1;
        ExoVM *exoVM = (__bridge ExoVM *)vm;
        return [exoVM stop] ? 0 : -1;
    }
}

int exo_vm_is_running(exo_vm_t vm) {
    @autoreleasepool {
        if (!vm) return 0;
        ExoVM *exoVM = (__bridge ExoVM *)vm;
        return exoVM.vm.state == VZVirtualMachineStateRunning ? 1 : 0;
    }
}

const char* exo_vm_last_error(exo_vm_t vm) {
    @autoreleasepool {
        if (!vm) return "null VM handle";
        ExoVM *exoVM = (__bridge ExoVM *)vm;
        return [exoVM lastErrorUTF8];
    }
}

int exo_vm_rpc_fds(exo_vm_t vm, int *read_fd, int *write_fd) {
    @autoreleasepool {
        if (!vm || !read_fd || !write_fd) return -1;
        ExoVM *exoVM = (__bridge ExoVM *)vm;
        if (exoVM.rpcHostReadFd < 0 || exoVM.rpcHostWriteFd < 0) {
            return -1;
        }
        *read_fd = exoVM.rpcHostReadFd;
        *write_fd = exoVM.rpcHostWriteFd;
        return 0;
    }
}

int exo_vm_request(exo_vm_t vm, uint32_t port, const char *json_in,
                   char **json_out, uint32_t timeout_ms) {
    @autoreleasepool {
        if (!vm || !json_in || !json_out) return -1;
        ExoVM *exoVM = (__bridge ExoVM *)vm;
        if (exoVM.vm.state != VZVirtualMachineStateRunning) {
            [exoVM setLastErrorFromError:nil message:@"VM is not running"];
            return -1;
        }

        NSError *err = nil;
        NSString *response = [exoVM requestOnPort:port
                                            json:[NSString stringWithUTF8String:json_in]
                                         timeout:timeout_ms
                                           error:&err];
        if (err) {
            [exoVM setLastErrorFromError:err message:@"Guest request failed"];
            return -1;
        }
        if (!response) {
            [exoVM setLastErrorFromError:nil message:@"Empty guest response"];
            return -1;
        }

        *json_out = strdup([response UTF8String]);
        return 0;
    }
}

void exo_vm_free_string(char *s) {
    if (s) free(s);
}
