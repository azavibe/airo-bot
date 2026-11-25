import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import './index.css';
import { applyTheme, loadCachedTheme } from './utils/theme';
import { startLanguage } from './i18n';

// Paint the last-used theme before React renders, so there is no flash of
// the default colours while /api/me loads the account's saved theme.
applyTheme(loadCachedTheme());

// The same for the language: the words load (a small local file) before the
// first paint, so English doesn't flash before the person's language.
void startLanguage().finally(() => {
  ReactDOM.createRoot(document.getElementById('root')!).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>
  );
});
