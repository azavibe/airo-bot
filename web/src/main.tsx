import React from 'react';
import ReactDOM from 'react-dom/client';
import { App } from './App';
import './index.css';
import { applyTheme, loadCachedTheme } from './utils/theme';

// Paint the last-used theme before React renders, so there is no flash of
// the default colours while /api/me loads the account's saved theme.
applyTheme(loadCachedTheme());

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
