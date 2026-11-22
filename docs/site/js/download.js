// Primary "Download" button in the hero: points it at the release file for the visitor's
// system and CPU. The URLs are injected by scripts/build-site.py into data-urls on #dl-main
// as JSON, keyed "windows-x64", "windows-arm64", "linux-x64", "linux-arm64" and "mac".

const OS_NAMES = {
  windows: 'Windows',
  linux: 'Linux',
  mac: 'macOS',
};

/**
 * Guess the operating system from the user agent string.
 * @returns {'windows' | 'mac' | 'linux'}
 */
function detectOs() {
  const userAgent = navigator.userAgent;
  if (/Windows/.test(userAgent)) {
    return 'windows';
  }
  const isMac = /Mac OS X|Macintosh/.test(userAgent);
  const isMobile = /Mobile/.test(userAgent);
  if (isMac && !isMobile) {
    return 'mac';
  }
  return 'linux';
}

/**
 * Quick CPU guess from the user agent string (Linux browsers often name the CPU there).
 * @returns {'arm64' | 'x64'}
 */
function detectArchFromUserAgent() {
  if (/aarch64|arm64|armv8/i.test(navigator.userAgent)) {
    return 'arm64';
  }
  return 'x64';
}

/**
 * Ask the browser for the CPU architecture (User-Agent Client Hints, Chromium only).
 * Resolves to null unless the browser reports a 64-bit Arm CPU.
 * @returns {Promise<'arm64' | null>}
 */
async function detectArch() {
  const userAgentData = navigator.userAgentData;
  if (!userAgentData || !userAgentData.getHighEntropyValues) {
    return null;
  }
  try {
    const values = await userAgentData.getHighEntropyValues(['architecture', 'bitness']);
    if (values.architecture === 'arm' && values.bitness !== '32') {
      return 'arm64';
    }
  } catch {
    // The browser refused; keep the first guess.
  }
  return null;
}

/**
 * The download URL for a system and CPU, or undefined when the release has no such file.
 * @param {Record<string, string>} urls
 * @param {string} os
 * @param {string} arch
 * @returns {string | undefined}
 */
function pickAsset(urls, os, arch) {
  if (os === 'mac') {
    return urls.mac;
  }
  return urls[`${os}-${arch}`];
}

/**
 * Update the button, the file name next to the version and, on arm64, the "Also:" links.
 * @param {HTMLAnchorElement} button
 * @param {Record<string, string>} urls
 * @param {string} os
 * @param {string} arch
 */
function render(button, urls, os, arch) {
  const url = pickAsset(urls, os, arch);
  if (url) {
    button.href = url;
  }

  let label = `Download for ${OS_NAMES[os]}`;
  if (arch === 'arm64' && os !== 'mac') {
    label += ' (Arm)';
  }
  button.querySelector('span').textContent = label;

  const fileName = decodeURIComponent(button.href.split('/').pop());
  document.getElementById('dl-for').textContent = ` · ${fileName}`;

  if (arch === 'arm64') {
    for (const link of document.querySelectorAll('#dl-alt [data-arm]')) {
      if (link.dataset.arm) {
        link.href = link.dataset.arm;
      }
    }
  }
}

function init() {
  const button = document.getElementById('dl-main');
  if (!button) {
    return;
  }
  const urls = JSON.parse(button.dataset.urls || '{}');
  const os = detectOs();
  const firstGuess = detectArchFromUserAgent();
  render(button, urls, os, firstGuess);

  detectArch().then((arch) => {
    if (arch === 'arm64' && firstGuess !== 'arm64') {
      render(button, urls, os, arch);
    }
  });
}

init();
