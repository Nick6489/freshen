// A minimal stand-in for XCTest so the test sources can be syntax-checked without
// a macOS SDK (see Tools/check-syntax.sh). The assertion macros evaluate their
// arguments for type checking but assert nothing. Run the real tests on macOS.
#ifndef FRESHEN_STUB_XCTEST_H
#define FRESHEN_STUB_XCTEST_H

#import <Foundation/Foundation.h>

@interface XCTestExpectation : NSObject
- (void)fulfill;
@end

@interface XCTestCase : NSObject
- (XCTestExpectation *)expectationWithDescription:(NSString *)description;
- (void)waitForExpectations:(NSArray<XCTestExpectation *> *)expectations timeout:(NSTimeInterval)seconds;
@end

static inline void FSHStubUse(int unused, ...) { (void)unused; }

#define XCTAssertTrue(expr, ...) do { BOOL fsh_v = (BOOL)(expr); FSHStubUse(0, fsh_v, ##__VA_ARGS__); } while (0)
#define XCTAssertFalse(expr, ...) do { BOOL fsh_v = (BOOL)(expr); FSHStubUse(0, fsh_v, ##__VA_ARGS__); } while (0)
#define XCTAssertNil(expr, ...) do { id fsh_v = (id)(expr); FSHStubUse(0, fsh_v, ##__VA_ARGS__); } while (0)
#define XCTAssertNotNil(expr, ...) do { id fsh_v = (id)(expr); FSHStubUse(0, fsh_v, ##__VA_ARGS__); } while (0)
#define XCTAssertEqualObjects(a, b, ...) do { id fsh_a = (id)(a); id fsh_b = (id)(b); FSHStubUse(0, fsh_a, fsh_b, ##__VA_ARGS__); } while (0)
#define XCTAssertEqual(a, b, ...) do { __typeof__(a) fsh_a = (a); __typeof__(b) fsh_b = (b); (void)fsh_a; (void)fsh_b; FSHStubUse(0, ##__VA_ARGS__); } while (0)
#define XCTAssertGreaterThan(a, b, ...) do { __typeof__(a) fsh_a = (a); __typeof__(b) fsh_b = (b); (void)fsh_a; (void)fsh_b; FSHStubUse(0, ##__VA_ARGS__); } while (0)
#define XCTFail(...) FSHStubUse(0, ##__VA_ARGS__)

#endif
