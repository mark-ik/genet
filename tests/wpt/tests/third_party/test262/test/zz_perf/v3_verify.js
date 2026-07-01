/*---
includes: [propertyHelper.js]
---*/
var a=[0,1];
Object.defineProperty(a,"1",{value:1,configurable:false});
verifyProperty(a,"length",{value:2,writable:true,configurable:false,enumerable:false});
