// hook_hid.m — interposes IOHIDManager/IOHIDDevice calls in emWave Pro.app
// to log the exact HID traffic (both directions) the app exchanges with the
// emWave2. The app's entitlements (allow-dyld-environment-variables +
// disable-library-validation) permit DYLD_INSERT_LIBRARIES injection.
//
// Build:
//   clang -dynamiclib -fobjc-arc -framework IOKit -framework CoreFoundation \
//         -o hook_hid.dylib hook_hid.m
// Run:
//   DYLD_INSERT_LIBRARIES=/path/hook_hid.dylib "/Applications/emWave Pro.app/Contents/MacOS/emWaveMac"
//
// Log: /tmp/hid_hook.log (append)

#import <IOKit/hid/IOHIDManager.h>
#import <IOKit/hid/IOHIDDevice.h>
#import <pthread.h>
#import <stdio.h>
#import <stdarg.h>
#import <time.h>
#import <string.h>

static FILE *g_log = NULL;
static pthread_mutex_t g_mu = PTHREAD_MUTEX_INITIALIZER;

static void hlog(const char *fmt, ...) {
    pthread_mutex_lock(&g_mu);
    if (!g_log) g_log = fopen("/tmp/hid_hook.log", "a");
    if (g_log) {
        struct timespec ts; clock_gettime(CLOCK_REALTIME, &ts);
        double t = ts.tv_sec + ts.tv_nsec / 1e9;
        fprintf(g_log, "[%12.3f] ", t);
        va_list ap; va_start(ap, fmt);
        vfprintf(g_log, fmt, ap);
        va_end(ap);
        fflush(g_log);
    }
    pthread_mutex_unlock(&g_mu);
}

static void dump_bytes(const char *tag, const uint8_t *p, CFIndex n) {
    hlog("%s (%ld B): ", tag, (long)n);
    for (CFIndex i = 0; i < n && i < 64; i++) fprintf(g_log, "%02x ", p[i]);
    fprintf(g_log, "\n");
    fflush(g_log);
}

// ---- input report callback wrapper -----------------------------------
typedef void (*IOHIDReportCallbackT)(void *context, IOReturn result, void *sender,
                                     IOHIDReportType type, uint32_t reportID,
                                     uint8_t *report, CFIndex reportLength);

typedef struct {
    IOHIDReportCallbackT orig;
    void *ctx;
} WrapCtx;

static void wrapped_input_cb(void *context, IOReturn result, void *sender,
                             IOHIDReportType type, uint32_t reportID,
                             uint8_t *report, CFIndex reportLength) {
    WrapCtx *w = (WrapCtx *)context;
    dump_bytes("IN", report, reportLength);
    if (w->orig) w->orig(w->ctx, result, sender, type, reportID, report, reportLength);
    free(w);
}

// ---- interposed functions ----------------------------------------------
void my_IOHIDDeviceRegisterInputReportCallback(
    IOHIDDeviceRef device, uint8_t *buffer, CFIndex bufferSize,
    IOHIDReportCallbackT callback, void *context) {
    hlog("IOHIDDeviceRegisterInputReportCallback bufSize=%ld\n", (long)bufferSize);
    WrapCtx *w = calloc(1, sizeof(WrapCtx));
    w->orig = callback;
    w->ctx = context;
    IOHIDDeviceRegisterInputReportCallback(device, buffer, bufferSize,
                                                  wrapped_input_cb, w);
}

IOReturn my_IOHIDDeviceSetReport(IOHIDDeviceRef device, IOHIDReportType type,
                                 CFIndex reportID, const uint8_t *report,
                                 CFIndex reportLength) {
    hlog("SET type=%d id=%ld ", (int)type, (long)reportID);
    dump_bytes("OUT", report, reportLength);
    return IOHIDDeviceSetReport(device, type, reportID, report, reportLength);
}

IOReturn my_IOHIDDeviceGetReport(IOHIDDeviceRef device, IOHIDReportType type,
                                 CFIndex reportID, uint8_t *report,
                                 CFIndex *reportLength) {
    IOReturn r = IOHIDDeviceGetReport(device, type, reportID, report, reportLength);
    hlog("GET type=%d id=%ld -> %d", (int)type, (long)reportID, (int)r);
    if (r == kIOReturnSuccess && reportLength) dump_bytes("FEAT", report, *reportLength);
    return r;
}

IOReturn my_IOHIDDeviceOpen(IOHIDDeviceRef device, IOOptionBits options) {
    hlog("IOHIDDeviceOpen options=0x%x\n", (unsigned)options);
    return IOHIDDeviceOpen(device, options);
}

IOReturn my_IOHIDDeviceClose(IOHIDDeviceRef device, IOOptionBits options) {
    hlog("IOHIDDeviceClose options=0x%x\n", (unsigned)options);
    return IOHIDDeviceClose(device, options);
}

// ---- dyld interpose table ---------------------------------------------
typedef struct interpose_s { const void *replacement; const void *replacee; } interpose_t;
static const interpose_t interposers[] __attribute__((section("__DATA,__interpose"))) = {
    { (const void *)&my_IOHIDDeviceRegisterInputReportCallback,
      (const void *)&IOHIDDeviceRegisterInputReportCallback },
    { (const void *)&my_IOHIDDeviceSetReport, (const void *)&IOHIDDeviceSetReport },
    { (const void *)&my_IOHIDDeviceGetReport, (const void *)&IOHIDDeviceGetReport },
    { (const void *)&my_IOHIDDeviceOpen, (const void *)&IOHIDDeviceOpen },
    { (const void *)&my_IOHIDDeviceClose, (const void *)&IOHIDDeviceClose },
};
