#include <errno.h>
#include <fcntl.h>
#include <linux/hidraw.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <umockdev.h>

#define HIDRAW "/dev/hidraw63"
#define HID_SYSFS "/devices/virtual/hid/0003:046D:C095.0001"
// Vendor usage page 0xff00, HID++ long usage 2, report ID 0x11.
#define REPORT_DESCRIPTOR "0600FF0902A101851175089513810095139100C0"
#define LONG_REPORT_SIZE 20
#define SHORT_REPORT_SIZE 7

static const char device[] =
    "P: " HID_SYSFS "\n"
    "E: SUBSYSTEM=hid\n"
    "E: HID_ID=0003:0000046D:0000C095\n"
    "E: HID_NAME=Logitech G502 X PLUS\n"
    "E: HID_UNIQ=\n"
    "H: report_descriptor=" REPORT_DESCRIPTOR "\n"
    "\n"
    "P: " HID_SYSFS "/hidraw/hidraw63\n"
    "E: SUBSYSTEM=hidraw\n"
    "E: DEVNAME=" HIDRAW "\n"
    "E: MAJOR=240\n"
    "E: MINOR=63\n"
    "A: dev=240:63\n"
    "N: hidraw63\n"
    "L: device=../..\n";

static gboolean descriptor_size(UMockdevIoctlBase *handler,
                                UMockdevIoctlClient *client, gpointer unused)
{
    (void)handler;
    (void)unused;
    if (umockdev_ioctl_client_get_request(client) != HIDIOCGRDESCSIZE)
        return FALSE;

    int size = (sizeof(REPORT_DESCRIPTOR) - 1) / 2;
    g_autoptr(GError) error = NULL;
    g_autoptr(UMockdevIoctlData) data = umockdev_ioctl_data_resolve(
        umockdev_ioctl_client_get_arg(client), 0, sizeof(size), &error);
    if (!data) {
        fprintf(stderr, "HIDIOCGRDESCSIZE: %s\n", error->message);
        umockdev_ioctl_client_complete(client, -1, EIO);
    } else {
        umockdev_ioctl_data_update(data, 0, (guint8 *)&size, sizeof(size));
        umockdev_ioctl_client_complete(client, 0, 0);
    }
    return TRUE;
}

static void reply(const uint8_t *request, uint8_t *response, uint8_t percentage)
{
    enum { UNIFIED_BATTERY = 6, DEVICE_NAME = 7, DEVICE_INFO = 8 };
    static const char name[] = "Logitech G502 X PLUS";
    static const uint8_t identity[] = {
        0, 0x12, 0x34, 0x56, 0x78, 0, 0, 0x40, 0x99, 0xc0, 0x95, 0, 0
    };
    const uint8_t feature = request[2];
    const uint8_t function = request[3] >> 4;
    uint8_t *params = response + 4;
    response[0] = 0x11;
    memcpy(response + 1, request + 1, 3);

    if (request[1] == 0xff) {
        if (feature == 0 && function == 0) {
            const unsigned logical_feature = (request[4] << 8) | request[5];
            switch (logical_feature) {
            case 0x1004: params[0] = UNIFIED_BATTERY; break;
            case 0x0005: params[0] = DEVICE_NAME; break;
            case 0x0003:
                params[0] = DEVICE_INFO;
                params[2] = 1; // DEVICE INFORMATION feature version
                break;
            }
            return;
        }
        if (feature == UNIFIED_BATTERY && function == 0) {
            params[0] = 0x0f; // Supports all four battery levels.
            params[1] = 0x03; // Rechargeable and reports an exact percentage.
            return;
        }
        if (feature == UNIFIED_BATTERY && function == 1) {
            params[0] = percentage;
            params[1] = percentage <= 40 ? 0x02 : 0x04;
            return;
        }
        if (feature == DEVICE_NAME && function == 0) {
            params[0] = sizeof(name) - 1;
            return;
        }
        if (feature == DEVICE_NAME && function == 1 && request[4] < sizeof(name) - 1) {
            const size_t remaining = sizeof(name) - 1 - request[4];
            memcpy(params, name + request[4], MIN(remaining, LONG_REPORT_SIZE - 4));
            return;
        }
        if (feature == DEVICE_INFO && function == 0) {
            memcpy(params, identity, sizeof(identity));
            return;
        }
    }
    response[2] = 0xff; // HID++ 2.0 error response.
    response[3] = feature;
    response[4] = request[3];
    response[5] = 0x02; // Invalid argument.
}

static int serve(int hid_fd, const char *root)
{
    // Keep the slave open across the app's short-lived HID sessions so the
    // master never signals a hangup between battery reads.
    g_autofree char *slave_name = g_strdup(ptsname(hid_fd));
    int slave = slave_name ? open(slave_name, O_RDWR | O_NOCTTY | O_CLOEXEC) : -1;
    if (slave < 0) {
        perror("open simulated HID PTY slave");
        return 1;
    }

    puts(root);
    fflush(stdout);

    // The parent owns stdin and closes it to stop the simulator.
    struct pollfd fds[] = {{STDIN_FILENO, POLLIN, 0}, {hid_fd, POLLIN, 0}};
    uint8_t percentage = 63;
    uint8_t request[LONG_REPORT_SIZE];
    size_t used = 0;
    int status = 0;
    for (;;) {
        if (poll(fds, 2, -1) < 0) {
            if (errno == EINTR)
                continue;
            perror("poll HID simulator");
            status = 1;
            break;
        }
        if (fds[0].revents & (POLLIN | POLLHUP)) {
            ssize_t count = read(STDIN_FILENO, &percentage, 1);
            if (count != 1) {
                status = count < 0;
                break;
            }
            puts("ok");
            fflush(stdout);
        }
        if (!(fds[1].revents & POLLIN))
            continue;
        // A PTY is a byte stream, so assemble one report even if read splits it.
        size_t report_size = 1;
        if (used > 0)
            report_size = request[0] == 0x10 ? SHORT_REPORT_SIZE : LONG_REPORT_SIZE;
        ssize_t count = read(hid_fd, request + used, report_size - used);
        if (count <= 0 || (request[0] != 0x10 && request[0] != 0x11)) {
            fprintf(stderr, "Could not read a HID++ request from the simulated device\n");
            status = 1;
            break;
        }
        used += (size_t)count;
        if (report_size == 1 || used != report_size)
            continue;

        uint8_t response[LONG_REPORT_SIZE] = {0};
        reply(request, response, percentage);
        if (write(hid_fd, response, sizeof(response)) != sizeof(response)) {
            perror("write simulated HID++ response");
            status = 1;
            break;
        }
        used = 0;
    }
    close(slave);
    return status;
}

int main(void)
{
    g_autoptr(UMockdevTestbed) testbed = umockdev_testbed_new();
    g_autoptr(UMockdevIoctlBase) handler = umockdev_ioctl_base_new();
    g_autoptr(GError) error = NULL;
    g_signal_connect(handler, "handle-ioctl", G_CALLBACK(descriptor_size), NULL);
    if (!umockdev_testbed_add_from_string(testbed, device, &error) ||
        !umockdev_testbed_attach_ioctl(testbed, HIDRAW, handler, &error)) {
        fprintf(stderr, "Could not create simulated HID device: %s\n", error->message);
        return 1;
    }
    const int fd = umockdev_testbed_get_dev_fd(testbed, HIDRAW);
    if (fd < 0) {
        fprintf(stderr, "Could not get simulated HID device descriptor\n");
        return 1;
    }

    g_autofree char *root = umockdev_testbed_get_root_dir(testbed);
    return serve(fd, root);
}
