/// Authenticated desktop application updates for Objective-C, over the freshen C ABI.
///
/// This layer adds no update logic. Signature verification, bounded extraction, the
/// helper process and rollback all run in the native library.
#import <Foundation/Foundation.h>

#import "FSHError.h"
#import "FSHHost.h"
#import "FSHTransport.h"
#import "FSHTypes.h"
#import "FSHUpdater.h"
