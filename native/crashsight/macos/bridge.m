#import <Foundation/Foundation.h>
#import <CrashSight/CrashSight.h>
#include <stdint.h>

// Build against the official macOS SDK headers, never guessed objc_msgSend ABIs.
// Do not unload this bridge/framework after CrashSight installs crash handlers.
__attribute__((visibility("default"))) uint32_t tdn_cs_abi_version(void) { return 1; }

__attribute__((visibility("default"))) int tdn_cs_init(const char *app_id, const char *server, const char *version) {
    @autoreleasepool {
        @try {
            if (![NSThread isMainThread]) return -1;
            NSString *appId = [NSString stringWithUTF8String:app_id];
            NSString *url = [NSString stringWithUTF8String:server];
            NSString *appVersion = [NSString stringWithUTF8String:version];
            if (!appId || !url || !appVersion) return -1;
            CrashSightConfig *config = [[CrashSightConfig alloc] init];
            config.crashServerUrl = url;
            config.debugMode = NO;
            [CrashSight startWithAppId:appId developmentDevice:NO config:config];
            [CrashSight updateAppVersion:appVersion];
            return 0;
        } @catch (NSException *exception) { return -1; }
    }
}
__attribute__((visibility("default"))) void tdn_cs_set_value(const char *key, const char *value) {
    @autoreleasepool {
        @try {
            NSString *k = [NSString stringWithUTF8String:key];
            NSString *v = [NSString stringWithUTF8String:value];
            if (k && v) [CrashSight setUserValue:v forKey:k];
        } @catch (NSException *exception) { }
    }
}
__attribute__((visibility("default"))) int tdn_cs_report(const char *name, const char *message, const char *stack, const char *extras) {
    @autoreleasepool {
        @try {
            NSString *n = [NSString stringWithUTF8String:name];
            NSString *m = [NSString stringWithUTF8String:message];
            NSString *s = [NSString stringWithUTF8String:stack];
            NSString *e = [NSString stringWithUTF8String:extras];
            if (!n || !m || !s || !e) return -1;
            id info = [NSJSONSerialization JSONObjectWithData:[e dataUsingEncoding:NSUTF8StringEncoding] options:0 error:nil];
            if (![info isKindOfClass:[NSDictionary class]]) return -1;
            [CrashSight reportExceptionWithCategory:3 name:n reason:m
                callStack:s.length ? [s componentsSeparatedByString:@"\n"] : @[]
                extraInfo:info terminateApp:NO dumpDataType:0 errorAttachmentPath:nil];
            return 0;
        } @catch (NSException *exception) { return -1; }
    }
}
