config => {
  if (location.origin !== config.origin) throw new Error('Unexpected login origin');
  const contacts = document.body.innerText.match(/[a-zA-Z0-9.!#$%&'*+\/=?^_`{|}~-]+@[a-zA-Z0-9.-]+/g) || [];
  if (!contacts.some(email => email.toLowerCase() === config.email)) throw new Error('Unexpected login recipient');
  const inputs = [...document.querySelectorAll(config.selector)].filter(input => input instanceof HTMLInputElement && input.getClientRects().length > 0);
  if (inputs.length !== 1 || inputs[0].disabled || inputs[0].readOnly) throw new Error('Verification input unavailable');
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(inputs[0], config.code);
  inputs[0].dispatchEvent(new Event('input', {bubbles:true}));
  inputs[0].dispatchEvent(new Event('change', {bubbles:true}));
  return {codeEntered:true};
}
