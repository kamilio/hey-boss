// Run by Apple's signed osascript, whose Keychain identity survives CLI upgrades.
// Credential data crosses stdin/stdout only. This script never logs a request.
ObjC.import('Foundation');
ObjC.import('Security');
function run() {
  $.SecKeychainSetUserInteractionAllowed(false);
  const input = $.NSFileHandle.fileHandleWithStandardInput.readDataToEndOfFile;
  const request = JSON.parse(ObjC.unwrap($.NSString.alloc.initWithDataEncoding(input, $.NSUTF8StringEncoding)));
  const query = $({ class: 'genp', svce: request.service, acct: request.account });
  const dict = query.mutableCopy;
  dict.setObjectForKey($(true), $('r_Data'));
  const output = Ref();
  const status = $.SecItemCopyMatching(dict, output);
  if (status !== 0) return JSON.stringify({ status });
  const data = ObjC.castRefToObject(output[0]);
  return JSON.stringify({ status: 0, data: ObjC.unwrap(data.base64EncodedStringWithOptions(0)) });
}
