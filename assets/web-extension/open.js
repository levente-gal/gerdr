// The worker sends this tab on to its page (see worker.js); should it not
// get to it, the page goes there itself.
setTimeout(() => {
  const url = new URLSearchParams(location.search).get("url");
  if (url) location.replace(url);
}, 2000);
