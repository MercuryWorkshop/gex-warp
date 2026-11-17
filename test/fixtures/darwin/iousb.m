// darwin/devices-page's second program: IOUSBHost over the page's
// `devices` service, as a Mac program uses it, built against the SDK's
// IOUSBHost. The page gives it a USB device of a vendor's class (an
// interface of a bulk endpoint each way and an interrupt one in); it finds
// the device and its interface by matching dictionaries IOUSBHost makes,
// reads their descriptors and strings, makes vendor requests each way,
// writes to the bulk pipe out and reads back what the device made of it
// (one request waited for, one enqueued), hears the interrupt pipe stall
// and clears it, is refused what is not offered, lets the interface and
// the device go; then opens the device again and asks it to leave, and
// hears that it has.
#import <Foundation/Foundation.h>
#import <IOUSBHost/IOUSBHost.h>
#include <stdio.h>
#include <unistd.h>

static void error(const char *what, NSError *e) { printf("%s: %s %#lx\n", what, e.domain.UTF8String, (long)e.code); }

static void hex(const char *label, const void *p, NSUInteger n) {
    printf("%s:", label);
    for (NSUInteger i = 0; i < n; i++) printf(" %02x", ((const uint8_t *)p)[i]);
    printf("\n");
}

int main(void) {
    @autoreleasepool {
        setvbuf(stdout, NULL, _IOLBF, 0);
        alarm(120);
        io_service_t s = IOServiceGetMatchingService(kIOMainPortDefault, [IOUSBHostDevice createMatchingDictionaryWithVendorID:@0x1209 productID:@2 bcdDevice:nil deviceClass:nil
                                                                                                   deviceSubclass:nil deviceProtocol:nil speed:nil productIDArray:nil]);
        printf("the device %d\n", s != 0);
        printf("by its product among others %d\n", IOServiceGetMatchingService(kIOMainPortDefault, [IOUSBHostDevice createMatchingDictionaryWithVendorID:@0x1209 productID:nil
                                                                                    bcdDevice:nil deviceClass:nil deviceSubclass:nil deviceProtocol:nil speed:nil productIDArray:@[ @7, @2 ]]) != 0);
        printf("not by another's %d\n", IOServiceGetMatchingService(kIOMainPortDefault, [IOUSBHostDevice createMatchingDictionaryWithVendorID:@0x1209 productID:nil bcdDevice:nil
                                                                     deviceClass:nil deviceSubclass:nil deviceProtocol:nil speed:nil productIDArray:@[ @7, @8 ]]) != 0);
        NSError *e = nil;
        IOUSBHostDevice *dev = [[IOUSBHostDevice alloc] initWithIOService:s options:IOUSBHostObjectInitOptionsNone queue:nil error:&e interestHandler:nil];
        printf("opened %d\n", dev != nil);
        const IOUSBDeviceDescriptor *dd = dev.deviceDescriptor;
        printf("device descriptor: USB %#x class %d vendor %#x product %#x release %#x strings %d %d %d, %d configuration\n", USBToHost16(dd->bcdUSB), dd->bDeviceClass,
               USBToHost16(dd->idVendor), USBToHost16(dd->idProduct), USBToHost16(dd->bcdDevice), dd->iManufacturer, dd->iProduct, dd->iSerialNumber, dd->bNumConfigurations);
        printf("strings: %s / %s / %s\n", [[dev stringWithIndex:dd->iManufacturer error:&e] UTF8String], [[dev stringWithIndex:dd->iProduct error:&e] UTF8String],
               [[dev stringWithIndex:dd->iSerialNumber error:&e] UTF8String]);
        const IOUSBConfigurationDescriptor *cd = dev.configurationDescriptor;
        printf("configuration %d of %d interfaces, %d bytes, %u mA\n", cd->bConfigurationValue, cd->bNumInterfaces, USBToHost16(cd->wTotalLength),
               IOUSBGetConfigurationMaxPowerMilliAmps(kIOUSBHostConnectionSpeedHigh, cd));
        printf("by its index the same %d\n", [dev configurationDescriptorWithIndex:0 error:&e] == cd);
        printf("no second: %d", [dev configurationDescriptorWithIndex:1 error:&e] == NULL);
        error("", e);
        const IOUSBInterfaceDescriptor *ifd = NULL;
        while ((ifd = IOUSBGetNextInterfaceDescriptor(cd, (const IOUSBDescriptorHeader *)ifd))) {
            printf("interface %d alternate %d class %#x/%#x/%#x:", ifd->bInterfaceNumber, ifd->bAlternateSetting, ifd->bInterfaceClass, ifd->bInterfaceSubClass, ifd->bInterfaceProtocol);
            const IOUSBEndpointDescriptor *ep = NULL;
            while ((ep = IOUSBGetNextEndpointDescriptor(cd, ifd, (const IOUSBDescriptorHeader *)ep)))
                printf(" %#x (type %d, %d bytes)", IOUSBGetEndpointAddress(ep), IOUSBGetEndpointType(ep), IOUSBGetEndpointMaxPacketSize(kIOUSBHostConnectionSpeedHigh, ep));
            printf("\n");
        }

        // Vendor requests, in and out.
        IOUSBDeviceRequest in = {IOUSBHostDeviceRequestType(kIOUSBDeviceRequestDirectionValueIn, kIOUSBDeviceRequestTypeValueVendor, kIOUSBDeviceRequestRecipientValueDevice), 1, 0x1234, 0, 16};
        NSMutableData *buf = [NSMutableData dataWithLength:16];
        NSUInteger n = 0;
        printf("vendor request in %d", [dev sendDeviceRequest:in data:buf bytesTransferred:&n completionTimeout:5 error:&e]);
        printf(", %lu bytes: %.*s\n", (unsigned long)n, (int)n, (const char *)buf.bytes);
        IOUSBDeviceRequest out = {IOUSBHostDeviceRequestType(kIOUSBDeviceRequestDirectionValueOut, kIOUSBDeviceRequestTypeValueVendor, kIOUSBDeviceRequestRecipientValueDevice), 2, 0, 0, 3};
        NSMutableData *three = [NSMutableData dataWithBytes:"\x01\x02\x03" length:3];
        printf("vendor request out %d", [dev sendDeviceRequest:out data:three bytesTransferred:&n completionTimeout:5 error:&e]);
        printf(", %lu bytes\n", (unsigned long)n);
        printf("configured %d\n", [dev configureWithValue:1 matchInterfaces:YES error:&e]);

        // The interface and its pipes.
        io_service_t is = IOServiceGetMatchingService(kIOMainPortDefault, [IOUSBHostInterface createMatchingDictionaryWithVendorID:@0x1209 productID:@2 bcdDevice:nil interfaceNumber:@0
                                                                                                  configurationValue:@1 interfaceClass:@0xff interfaceSubclass:nil interfaceProtocol:nil speed:nil
                                                                                                      productIDArray:nil]);
        printf("the interface %d\n", is != 0);
        IOUSBHostInterface *iface = [[IOUSBHostInterface alloc] initWithIOService:is options:IOUSBHostObjectInitOptionsNone queue:nil error:&e interestHandler:nil];
        printf("its descriptor: interface %d, %d endpoints\n", iface.interfaceDescriptor->bInterfaceNumber, iface.interfaceDescriptor->bNumEndpoints);
        IOUSBHostPipe *bulkOut = [iface copyPipeWithAddress:0x02 error:&e], *bulkIn = [iface copyPipeWithAddress:0x81 error:&e], *irq = [iface copyPipeWithAddress:0x83 error:&e];
        printf("pipes %#lx %#lx %#lx, of its interface %d\n", (unsigned long)bulkOut.endpointAddress, (unsigned long)bulkIn.endpointAddress, (unsigned long)irq.endpointAddress,
               bulkIn.hostInterface == iface);
        printf("no pipe 0x99: %d", [iface copyPipeWithAddress:0x99 error:&e] == nil);
        error("", e);
        NSMutableData *word = [NSMutableData dataWithBytes:"abcdef" length:6];
        printf("written %d", [bulkOut sendIORequestWithData:word bytesTransferred:&n completionTimeout:5 error:&e]);
        printf(", %lu bytes\n", (unsigned long)n);
        NSMutableData *back = [NSMutableData dataWithLength:16];
        printf("read %d", [bulkIn sendIORequestWithData:back bytesTransferred:&n completionTimeout:5 error:&e]);
        printf(", %lu bytes: %.*s\n", (unsigned long)n, (int)n, (const char *)back.bytes);
        // Enqueued: the completion on the interface's queue.
        dispatch_semaphore_t done = dispatch_semaphore_create(0);
        NSMutableData *more = [NSMutableData dataWithBytes:"xyz" length:3];
        [bulkOut enqueueIORequestWithData:more completionTimeout:5 error:&e completionHandler:^(IOReturn status, NSUInteger bytes) {
            printf("enqueued write: %#x, %lu bytes\n", status, (unsigned long)bytes);
            dispatch_semaphore_signal(done);
        }];
        // (Another pipe's requests are not in order with this one's.)
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        NSMutableData *again = [NSMutableData dataWithLength:8];
        [bulkIn enqueueIORequestWithData:again completionTimeout:5 error:&e completionHandler:^(IOReturn status, NSUInteger bytes) {
            printf("enqueued read: %#x, %lu bytes: %.*s\n", status, (unsigned long)bytes, (int)bytes, (const char *)again.bytes);
            dispatch_semaphore_signal(done);
        }];
        dispatch_semaphore_wait(done, DISPATCH_TIME_FOREVER);
        NSMutableData *irqData = [NSMutableData dataWithLength:8];
        printf("interrupt read %d", [irq sendIORequestWithData:irqData bytesTransferred:&n completionTimeout:5 error:&e]);
        error("", e);
        printf("stall cleared %d\n", [irq clearStallWithError:&e]);
        printf("alternate 0 %d, the same interface %d\n", [iface selectAlternateSetting:0 error:&e], IOObjectConformsTo(is, "IOUSBHostInterface"));
        printf("streams %d", [bulkIn enableStreamsWithError:&e]);
        error("", e);
        [iface destroy];
        [dev destroy];
        printf("let go\n");

        // Again, and the device leaves.
        dispatch_semaphore_t gone = dispatch_semaphore_create(0);
        IOUSBHostDevice *dev2 = [[IOUSBHostDevice alloc] initWithIOService:s options:IOUSBHostObjectInitOptionsNone queue:nil error:&e
                                                           interestHandler:^(IOUSBHostObject *o, uint32_t message, void *arg) {
                                                               printf("interest: %#x\n", message);
                                                               if (message == kIOMessageServiceIsTerminated) dispatch_semaphore_signal(gone);
                                                           }];
        IOUSBDeviceRequest leave = {IOUSBHostDeviceRequestType(kIOUSBDeviceRequestDirectionValueOut, kIOUSBDeviceRequestTypeValueVendor, kIOUSBDeviceRequestRecipientValueDevice), 0x7f, 0, 0, 0};
        printf("asked to leave %d\n", [dev2 sendDeviceRequest:leave error:&e]);
        printf("heard it leave %d\n", dispatch_semaphore_wait(gone, dispatch_time(DISPATCH_TIME_NOW, 30 * NSEC_PER_SEC)) == 0);
        printf("a request now %d", [dev2 sendDeviceRequest:in data:buf bytesTransferred:&n completionTimeout:5 error:&e]);
        error("", e);
        printf("no device now %d\n", IOServiceGetMatchingService(kIOMainPortDefault, [IOUSBHostDevice createMatchingDictionaryWithVendorID:@0x1209 productID:@2 bcdDevice:nil
                                                                   deviceClass:nil deviceSubclass:nil deviceProtocol:nil speed:nil productIDArray:nil]) == 0);
        [dev2 destroy];
    }
    return 0;
}
