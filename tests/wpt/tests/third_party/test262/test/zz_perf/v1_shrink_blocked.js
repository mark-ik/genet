/*---
flags: [raw]
---*/
var a=[0,1];
Object.defineProperty(a,"1",{value:1,configurable:false});
try{Object.defineProperty(a,"length",{value:1});}catch(e){}
