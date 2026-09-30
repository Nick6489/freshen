// A deliberately small stand-in for Apple's Foundation, used ONLY by
// Tools/check-syntax.sh so the Objective-C sources can be syntax-checked with
// ARC, blocks, nullability and -Werror on a machine that has no macOS SDK.
//
// It declares just the API the sources use, copied from the real SDK's
// signatures. It checks language-level correctness (syntax, ARC, blocks,
// nullability, designated initializers). It cannot check that the code uses
// Foundation correctly at runtime; only a build on macOS can.
#ifndef FRESHEN_STUB_FOUNDATION_H
#define FRESHEN_STUB_FOUNDATION_H

#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/types.h>

typedef signed char BOOL;
#define YES ((BOOL)1)
#define NO ((BOOL)0)
#define nil ((void *)0)
#define Nil ((Class)0)

struct _NSZone;
struct NSFastEnumerationState;

typedef long NSInteger;
typedef unsigned long NSUInteger;
typedef double NSTimeInterval;

#define NS_ASSUME_NONNULL_BEGIN _Pragma("clang assume_nonnull begin")
#define NS_ASSUME_NONNULL_END _Pragma("clang assume_nonnull end")
#define NS_DESIGNATED_INITIALIZER __attribute__((objc_designated_initializer))
#define NS_UNAVAILABLE __attribute__((unavailable))
#define NS_REFINED_FOR_SWIFT __attribute__((swift_private))
#define FOUNDATION_EXPORT extern
#define NS_ENUM(_type, _name) enum _name : _type _name; enum _name : _type
#define NS_ERROR_ENUM(_domain, _name) NS_ENUM(NSInteger, _name)

@class NSString, NSNumber, NSURL, NSError, NSData, NSException;
@class NSDictionary<KeyType, ObjectType>;

@protocol NSObject
- (BOOL)isEqual:(nullable id)object;
- (BOOL)isKindOfClass:(Class)aClass;
@property(readonly) NSUInteger hash;
@end

@protocol NSCopying
- (id)copyWithZone:(nullable struct _NSZone *)zone;
@end

@interface NSObject <NSObject>
+ (instancetype)new;
+ (instancetype)alloc;
+ (Class)class;
- (instancetype)init;
- (id)copy;
@end

typedef NSString *NSErrorDomain;
typedef NSString *NSErrorUserInfoKey;
typedef NSString *NSStringEncodingName;
typedef NSUInteger NSStringEncoding;
#define NSUTF8StringEncoding ((NSStringEncoding)4)

@interface NSString : NSObject <NSCopying>
+ (nullable instancetype)stringWithUTF8String:(const char *)nullTerminatedCString;
+ (instancetype)stringWithFormat:(NSString *)format, ... __attribute__((format(__NSString__, 1, 2)));
@property(readonly, nullable) const char *UTF8String;
@property(readonly) NSUInteger length;
- (BOOL)isEqualToString:(NSString *)aString;
- (nullable NSData *)dataUsingEncoding:(NSStringEncoding)encoding;
@end

@interface NSNumber : NSObject
+ (NSNumber *)numberWithUnsignedLongLong:(unsigned long long)value;
+ (NSNumber *)numberWithBool:(BOOL)value;
@property(readonly) unsigned long long unsignedLongLongValue;
@end

@interface NSData : NSObject <NSCopying>
+ (instancetype)dataWithData:(NSData *)data;
+ (nullable instancetype)dataWithBytes:(nullable const void *)bytes length:(NSUInteger)length;
@property(readonly) NSUInteger length;
@property(readonly) const void *bytes;
@end

@interface NSMutableData : NSData
+ (nullable instancetype)dataWithCapacity:(NSUInteger)aNumItems;
+ (nullable instancetype)dataWithLength:(NSUInteger)length;
@property(readonly) void *mutableBytes;
- (void)appendBytes:(const void *)bytes length:(NSUInteger)length;
- (void)appendData:(NSData *)other;
@end

@interface NSURL : NSObject <NSCopying>
+ (nullable instancetype)URLWithString:(NSString *)URLString;
+ (instancetype)fileURLWithPath:(NSString *)path;
@property(readonly, copy, nullable) NSString *path;
@property(readonly, copy, nullable) NSString *scheme;
@property(readonly, copy, nullable) NSString *absoluteString;
@property(readonly, getter=isFileURL) BOOL fileURL;
@end

FOUNDATION_EXPORT NSErrorDomain const NSCocoaErrorDomain;
FOUNDATION_EXPORT NSErrorUserInfoKey const NSLocalizedDescriptionKey;
FOUNDATION_EXPORT NSErrorUserInfoKey const NSUnderlyingErrorKey;
#define NSUserCancelledError 3072

@interface NSError : NSObject <NSCopying>
+ (instancetype)errorWithDomain:(NSErrorDomain)domain
                           code:(NSInteger)code
                       userInfo:(nullable NSDictionary<NSErrorUserInfoKey, id> *)dict;
@property(readonly, copy) NSErrorDomain domain;
@property(readonly) NSInteger code;
@property(readonly, copy) NSString *localizedDescription;
@property(readonly, copy) NSDictionary<NSErrorUserInfoKey, id> *userInfo;
@end

@interface NSException : NSObject
@property(readonly, copy) NSString *name;
@property(readonly, copy, nullable) NSString *reason;
@end

@interface NSArray<__covariant ObjectType> : NSObject <NSCopying>
+ (instancetype)array;
+ (instancetype)arrayWithObject:(ObjectType)anObject;
+ (instancetype)arrayWithObjects:(const ObjectType _Nonnull[_Nonnull])objects count:(NSUInteger)cnt;
@property(readonly) NSUInteger count;
@property(readonly, nullable) ObjectType firstObject;
@property(readonly, nullable) ObjectType lastObject;
- (ObjectType)objectAtIndex:(NSUInteger)index;
- (ObjectType)objectAtIndexedSubscript:(NSUInteger)idx;
- (BOOL)containsObject:(ObjectType)anObject;
- (NSUInteger)countByEnumeratingWithState:(struct NSFastEnumerationState *)state
                                  objects:(ObjectType __unsafe_unretained _Nullable[_Nullable])buffer
                                    count:(NSUInteger)len;
@end

@interface NSMutableArray<ObjectType> : NSArray<ObjectType>
+ (instancetype)arrayWithCapacity:(NSUInteger)numItems;
- (void)addObject:(ObjectType)anObject;
@end

@interface NSDictionary<__covariant KeyType, __covariant ObjectType> : NSObject <NSCopying>
+ (instancetype)dictionaryWithObject:(ObjectType)object forKey:(KeyType<NSCopying>)key;
- (nullable ObjectType)objectForKey:(KeyType)aKey;
- (nullable ObjectType)objectForKeyedSubscript:(KeyType)key;
@property(readonly) NSUInteger count;
@end

@interface NSMutableDictionary<KeyType, ObjectType> : NSDictionary<KeyType, ObjectType>
- (void)setObject:(ObjectType)anObject forKey:(KeyType<NSCopying>)aKey;
- (void)setObject:(nullable ObjectType)obj forKeyedSubscript:(KeyType<NSCopying>)key;
@end

@interface NSLock : NSObject
- (BOOL)tryLock;
- (void)unlock;
@end

// --- dispatch ---
typedef struct dispatch_queue_s *dispatch_queue_t;
typedef void (^dispatch_block_t)(void);
#define QOS_CLASS_USER_INITIATED 0x19
dispatch_queue_t dispatch_get_main_queue(void);
dispatch_queue_t dispatch_get_global_queue(long identifier, unsigned long flags);
void dispatch_async(dispatch_queue_t queue, dispatch_block_t block);
void dispatch_sync(dispatch_queue_t queue, dispatch_block_t block);
dispatch_queue_t dispatch_queue_create(const char *_Nullable label, const void *_Nullable attr);
typedef long dispatch_once_t;
void dispatch_once(dispatch_once_t *predicate, dispatch_block_t block);
typedef struct dispatch_semaphore_s *dispatch_semaphore_t;
typedef uint64_t dispatch_time_t;
#define DISPATCH_TIME_NOW ((dispatch_time_t)0)
#define DISPATCH_TIME_FOREVER (~(dispatch_time_t)0)
#define NSEC_PER_SEC 1000000000ull
dispatch_semaphore_t dispatch_semaphore_create(long value);
long dispatch_semaphore_wait(dispatch_semaphore_t dsema, dispatch_time_t timeout);
long dispatch_semaphore_signal(dispatch_semaphore_t dsema);
dispatch_time_t dispatch_time(dispatch_time_t when, int64_t delta);

// --- extra API used by the test sources ---
#define MIN(a, b) ((a) < (b) ? (a) : (b))

typedef struct _NSRange {
    NSUInteger location;
    NSUInteger length;
} NSRange;
static inline NSRange NSMakeRange(NSUInteger loc, NSUInteger len) {
    NSRange range = {loc, len};
    return range;
}

typedef NSString *NSExceptionName;
FOUNDATION_EXPORT NSExceptionName const NSInternalInconsistencyException;
#define NSFileReadNoSuchFileError 260

@interface NSException (Raise)
+ (void)raise:(NSExceptionName)name format:(NSString *)format, ... __attribute__((format(__NSString__, 2, 3)));
@end

@interface NSNumber (Stub)
+ (NSNumber *)numberWithUnsignedLong:(unsigned long)value;
@end

@interface NSString (Stub)
+ (nullable instancetype)stringWithContentsOfFile:(NSString *)path
                                         encoding:(NSStringEncoding)enc
                                            error:(NSError *_Nullable *_Nullable)error;
- (nullable instancetype)initWithData:(NSData *)data encoding:(NSStringEncoding)encoding;
- (BOOL)writeToFile:(NSString *)path
         atomically:(BOOL)useAuxiliaryFile
           encoding:(NSStringEncoding)enc
              error:(NSError *_Nullable *_Nullable)error;
- (NSString *)stringByAppendingPathComponent:(NSString *)str;
- (NSString *)stringByAppendingString:(NSString *)aString;
@end

@interface NSData (Stub)
+ (nullable instancetype)dataWithContentsOfFile:(NSString *)path;
+ (nullable instancetype)dataWithContentsOfFile:(NSString *)path
                                        options:(NSUInteger)readOptionsMask
                                          error:(NSError *_Nullable *_Nullable)errorPtr;
- (NSData *)subdataWithRange:(NSRange)range;
@end

@interface NSURL (Stub)
@property(readonly, copy, nullable) NSString *host;
@end

@interface NSDictionary<KeyType, ObjectType> (Literals)
+ (instancetype)dictionaryWithObjects:(const ObjectType _Nonnull[_Nullable])objects
                              forKeys:(const KeyType<NSCopying> _Nonnull[_Nullable])keys
                                count:(NSUInteger)cnt;
@end

@interface NSFileManager : NSObject
+ (NSFileManager *)defaultManager;
- (BOOL)createDirectoryAtPath:(NSString *)path
  withIntermediateDirectories:(BOOL)createIntermediates
                   attributes:(nullable NSDictionary<NSString *, id> *)attributes
                        error:(NSError *_Nullable *_Nullable)error;
- (nullable NSArray<NSString *> *)contentsOfDirectoryAtPath:(NSString *)path
                                                      error:(NSError *_Nullable *_Nullable)error;
@end
FOUNDATION_EXPORT NSString *NSTemporaryDirectory(void);

@interface NSUUID : NSObject
+ (instancetype)UUID;
@property(readonly, copy) NSString *UUIDString;
@end

@interface NSProcessInfo : NSObject
+ (NSProcessInfo *)processInfo;
@property(readonly, copy) NSDictionary<NSString *, NSString *> *environment;
@end

@interface NSDate : NSObject
+ (instancetype)dateWithTimeIntervalSinceNow:(NSTimeInterval)secs;
@property(readonly) NSTimeInterval timeIntervalSinceNow;
@end

@interface NSThread : NSObject
+ (void)sleepForTimeInterval:(NSTimeInterval)ti;
@end

@interface NSFileHandle : NSObject
- (NSData *)readDataToEndOfFile;
@end

@interface NSPipe : NSObject
+ (NSPipe *)pipe;
@property(readonly, retain) NSFileHandle *fileHandleForReading;
@end

@interface NSTask : NSObject
@property(nullable, copy) NSURL *executableURL;
@property(nullable, copy) NSArray<NSString *> *arguments;
@property(nullable, retain) id standardOutput;
@property(nullable, retain) id standardError;
@property(readonly) int terminationStatus;
- (BOOL)launchAndReturnError:(NSError *_Nullable *_Nullable)error;
- (void)waitUntilExit;
@end

#endif
