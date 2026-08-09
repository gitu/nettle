import { describe, expect, it } from 'vitest';
import { shortDir } from './util';

describe('shortDir', () => {
  it('shows the last path segment', () => {
    expect(shortDir('/home/deploy/apps/shop')).toBe('shop');
    expect(shortDir('/srv/nettle/')).toBe('nettle');
  });

  it('includes the parent when the last segment is too generic', () => {
    expect(shortDir('/home/deploy/shop/frontend')).toBe('shop/frontend');
    expect(shortDir('/opt/acme/api')).toBe('acme/api');
    expect(shortDir('/data/thing/BUILD')).toBe('thing/BUILD');
  });

  it('handles root and empty values', () => {
    expect(shortDir('/')).toBe('/');
    expect(shortDir(null)).toBeNull();
    expect(shortDir(undefined)).toBeNull();
    expect(shortDir('')).toBeNull();
  });

  it('keeps a bare generic name when there is no parent', () => {
    expect(shortDir('/frontend')).toBe('frontend');
  });
});
