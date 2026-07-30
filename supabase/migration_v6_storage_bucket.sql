-- ============================================================
-- Le Chariot – Schema v6 (Produktbilder in Supabase Storage)
-- Im Supabase SQL Editor ausführen.
-- Legt den öffentlichen Bucket `offer-images` an, in den der Push die
-- Produktbilder spiegelt. offers.image_url zeigt danach auf diesen Bucket
-- statt auf die rotierenden/hotlink-geschützten Händler-CDNs.
-- ============================================================

-- Öffentlicher Bucket für gespiegelte Produktbilder
insert into storage.buckets (id, name, public)
values ('offer-images', 'offer-images', true)
on conflict (id) do nothing;

-- Öffentliches Lesen (anon key der iOS-App). Schreiben läuft über den
-- Service-Role-Key, der Storage-RLS umgeht — keine Insert-Policy nötig.
drop policy if exists "Public read offer-images" on storage.objects;
create policy "Public read offer-images"
    on storage.objects for select
    using (bucket_id = 'offer-images');
