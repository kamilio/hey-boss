// Run by Apple's signed osascript, whose Keychain identity survives CLI upgrades.
// Credential data crosses stdin/stdout only. This script never logs a request.
ObjC.import('Foundation');
ObjC.import('Security');
function run() {
  $.SecKeychainSetUserInteractionAllowed(false);
  const input = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
  const request = JSON.parse(ObjC.unwrap($.NSString.alloc.initWithDataEncoding(input, $.NSUTF8StringEncoding)));
  const query = $({ class: 'genp', svce: request.service, acct: request.account });
  if (request.op === 'get') {
    const dict = query.mutableCopy;
    dict.setObjectForKey($(true), $('r_Data'));
    const output = Ref();
    const status = $.SecItemCopyMatching(dict, output);
    if (status !== 0) return JSON.stringify({ status });
    const data = ObjC.castRefToObject(output[0]);
    return JSON.stringify({ status: 0, data: ObjC.unwrap(data.base64EncodedStringWithOptions(0)) });
  }
  if (request.op === 'put') {
    const data = $.NSData.alloc.initWithBase64EncodedStringOptions($(request.data), 0);
    if (!data) return JSON.stringify({ status: -50 });
    const attrs = $({ v_Data: data });
    let status = $.SecItemUpdate(query, attrs);
    if (status === -25300) {
      const dict = query.mutableCopy;
      dict.setObjectForKey(data, $('v_Data'));
      status = $.SecItemAdd(dict, null);
    }
    return JSON.stringify({ status });
  }
  return JSON.stringify({ status: -50 });
}
