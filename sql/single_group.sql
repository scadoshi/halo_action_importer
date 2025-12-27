--dynamic

declare @num_groups int = 20;
declare @target_group int = 1;

with stats as (
    select 
        min(convert(int, cfactionid)) [min_id],
        max(convert(int, cfactionid)) [max_id],
        count(cfactionid) [total]
    from actions
    where cfactionid is not null
),
groups as (
    select 
        row_number() over (order by cfactionid) as row_num,
        cfactionid,
        (select total from stats) as total_count
    from actions
    where cfactionid is not null
),
grouped_ids as (
    select 
        ceiling(cast(row_num as float) / (cast(total_count as float) / @num_groups)) as group_num,
        cfactionid
    from groups
)
select 
    @target_group as group_num,
    stuff((
        select ',' + cfactionid
        from grouped_ids gi
        where gi.group_num = @target_group
        for xml path(''), type
    ).value('.', 'nvarchar(max)'), 1, 1, '') as action_ids;

