-- Le Chariot – Schema v6 (Produktbilder in Supabase Storage)

insert into storage.buckets (id, name, public)
values ('offer-images', 'offer-images', true)
on conflict (id) do nothing;

drop policy if exists "Public read offer-images" on storage.objects;
create policy "Public read offer-images"
    on storage.objects for select
    using (bucket_id = 'offer-images');
